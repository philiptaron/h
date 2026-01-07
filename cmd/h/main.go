// Usage: h <term>
//
// term can be of form [\w-]+ for search
// <user>/<repo> for github repos
// url for cloning
package main

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
)

const defaultCodeRoot = "~/src"

func main() {
	os.Exit(run())
}

func run() int {
	if len(os.Args) < 2 {
		return abort("Usage: h (<name> | <repo>/<name> | <url>) [git opts]")
	}

	term := os.Args[1]

	switch term {
	case "--setup":
		return setupMode()
	case "--resolve":
		return resolveMode()
	default:
		fmt.Fprintln(os.Stderr, "h is not installed")
		fmt.Fprintln(os.Stderr)
		fmt.Fprintln(os.Stderr, "h needs to be hooked into the shell before it can be used.")
		fmt.Fprintln(os.Stderr)
		return abort("Usage: eval \"$(h --setup [code-root])\"")
	}
}

func setupMode() int {
	codeRoot := defaultCodeRoot
	if len(os.Args) > 2 {
		codeRoot = os.Args[2]
	}
	codeRoot = expandPath(codeRoot)

	invokedPath, err := filepath.Abs(os.Args[0])
	if err != nil {
		invokedPath = os.Args[0]
	}

	fmt.Printf(`h() {
  _h_dir=$(command %s --resolve "%s" "$@")
  _h_ret=$?
  [ "$_h_dir" != "$PWD" ] && cd "$_h_dir"
  return $_h_ret
}
`, invokedPath, codeRoot)
	return 0
}

func resolveMode() int {
	if len(os.Args) < 3 {
		return abort("Usage: h (<name> | <repo>/<name> | <url>) [git opts]")
	}

	codeRoot := os.Args[2]

	if len(os.Args) < 4 {
		return abort("Usage: h (<name> | <repo>/<name> | <url>) [git opts]")
	}

	term := os.Args[3]
	gitOpts := os.Args[4:]

	switch term {
	case "-h", "--help":
		return abort("Usage: h (<name> | <repo>/<name> | <url>) [git opts]")
	}

	var path string
	var cloneURL string

	// Patterns
	githubRepoPattern := regexp.MustCompile(`^([\w.\-]+)/([\w.\-]+)$`)
	urlPattern := regexp.MustCompile(`://`)
	gitURLPattern := regexp.MustCompile(`^gite?a?@([^:]+):(.*)`)
	searchPattern := regexp.MustCompile(`^[\w.\-]+$`)

	switch {
	case githubRepoPattern.MatchString(term):
		// GitHub user/repo format
		matches := githubRepoPattern.FindStringSubmatch(term)
		owner, repo := matches[1], matches[2]

		// Query GitHub API for correct casing
		owner, repo = queryGitHubAPI(owner, repo)

		cloneURL = fmt.Sprintf("git@github.com:%s/%s.git", owner, repo)
		path = filepath.Join(codeRoot, "github.com", owner, repo)

	case urlPattern.MatchString(term):
		// Full URL
		u, err := url.Parse(term)
		if err != nil {
			return abort(fmt.Sprintf("Invalid URL: %s", term))
		}
		if u.Scheme == "" {
			return abort("Missing url scheme")
		}
		cloneURL = term
		urlPath := strings.TrimPrefix(u.Path, "/")
		path = filepath.Join(codeRoot, strings.ToLower(u.Host), urlPath)

	case gitURLPattern.MatchString(term):
		// git@host:path format
		matches := gitURLPattern.FindStringSubmatch(term)
		host, repoPath := matches[1], matches[2]
		cloneURL = term
		path = filepath.Join(codeRoot, host, repoPath)

	case searchPattern.MatchString(term):
		// Search for repo by name
		path = searchForRepo(codeRoot, term)
		if path == "" {
			return abort(fmt.Sprintf("%s not found", term))
		}

	default:
		return abort(fmt.Sprintf("Unknown pattern for %s", term))
	}

	if path == "" {
		return abort(fmt.Sprintf("%s not found", term))
	}

	// Remove .git extension from path
	path = strings.TrimSuffix(path, ".git")

	// Check if directory exists
	if info, err := os.Stat(path); err != nil || !info.IsDir() {
		// Clone the repository
		if cloneURL == "" {
			return abort(fmt.Sprintf("%s not found", term))
		}

		// Track the parent directory for cleanup
		parent := filepath.Dir(path)
		existingDir := parent
		for {
			if info, err := os.Stat(existingDir); err == nil && info.IsDir() {
				break
			}
			if existingDir == "/" || existingDir == "." {
				break
			}
			existingDir = filepath.Dir(existingDir)
		}

		// Create parent directory
		if err := os.MkdirAll(parent, 0755); err != nil {
			return abort(fmt.Sprintf("Failed to create directory: %v", err))
		}

		// Build git clone command
		args := []string{"clone"}
		if len(gitOpts) > 0 {
			args = append(args, gitOpts...)
		} else {
			args = append(args, "--recursive")
		}
		args = append(args, "--", cloneURL, path)

		cmd := exec.Command("git", args...)
		cmd.Stdout = os.Stderr
		cmd.Stderr = os.Stderr

		if err := cmd.Run(); err != nil {
			// Cleanup created directories
			for parent != existingDir {
				os.Remove(parent)
				parent = filepath.Dir(parent)
			}
			if exitErr, ok := err.(*exec.ExitError); ok {
				fmt.Println(getCurrentDir())
				return exitErr.ExitCode()
			}
			fmt.Println(getCurrentDir())
			return 1
		}
	}

	fmt.Println(path)
	return 0
}

func searchForRepo(codeRoot, term string) string {
	var bestPath string
	var bestDepth int

	caseSensitive := term != strings.ToLower(term)

	filepath.WalkDir(codeRoot, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}

		if !d.IsDir() {
			return nil
		}

		relPath, _ := filepath.Rel(codeRoot, path)
		depth := len(strings.Split(relPath, string(os.PathSeparator)))

		// Don't search below depth 4
		if depth > 4 {
			return fs.SkipDir
		}

		basename := filepath.Base(path)
		var match bool
		if caseSensitive {
			match = basename == term
		} else {
			match = strings.EqualFold(basename, term)
		}

		// Select deepest result
		if match && depth > bestDepth {
			bestPath = path
			bestDepth = depth
		}

		return nil
	})

	return bestPath
}

func queryGitHubAPI(owner, repo string) (string, string) {
	url := fmt.Sprintf("https://api.github.com/repos/%s/%s", owner, repo)
	resp, err := http.Get(url)
	if err != nil {
		return owner, repo
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return owner, repo
	}

	var result struct {
		Owner struct {
			Login string `json:"login"`
		} `json:"owner"`
		Name string `json:"name"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return owner, repo
	}

	return result.Owner.Login, result.Name
}

func expandPath(path string) string {
	if strings.HasPrefix(path, "~/") {
		home, err := os.UserHomeDir()
		if err == nil {
			return filepath.Join(home, path[2:])
		}
	}
	absPath, err := filepath.Abs(path)
	if err != nil {
		return path
	}
	return absPath
}

func getCurrentDir() string {
	dir, err := os.Getwd()
	if err != nil {
		return "."
	}
	return dir
}

func abort(msg string) int {
	fmt.Println(getCurrentDir())
	fmt.Fprintln(os.Stderr, msg)
	return 1
}
