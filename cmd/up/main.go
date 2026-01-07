// Usage: up
//
// Move back up to the project root.
//
// Up has a heuristic to detect the project root.
//
// If no project root is found it stays in the current directory
// If the current directory is already a project root it will try to find
// a parent project
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

var homeDir string

func init() {
	if h := os.Getenv("HOME"); h != "" {
		homeDir = h
	}
}

func main() {
	os.Exit(run())
}

func run() int {
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "-h", "--help":
			fmt.Fprintln(os.Stderr, "up is not installed")
			fmt.Fprintln(os.Stderr)
			return abort("Usage: eval \"$(up --setup)\"")
		case "--setup":
			fmt.Print(`up() {
  _up_dir=$(command up "$@")
  if [ $? = 0 ]; then
    [ "$_up_dir" != "$PWD" ] && cd "$_up_dir"
  fi
}
`)
			return 0
		}
	}

	pwd := os.Getenv("PWD")
	if pwd == "" {
		var err error
		pwd, err = os.Getwd()
		if err != nil {
			fmt.Println(".")
			return 1
		}
	}

	result := findProjectRoot(pwd)
	fmt.Println(result)
	return 0
}

func isProjectRoot(dir string) bool {
	// Check for .git directory
	if info, err := os.Stat(filepath.Join(dir, ".git")); err == nil && info.IsDir() {
		return true
	}

	// Check for .hg directory
	if info, err := os.Stat(filepath.Join(dir, ".hg")); err == nil && info.IsDir() {
		return true
	}

	// Check for .envrc file
	if info, err := os.Stat(filepath.Join(dir, ".envrc")); err == nil && !info.IsDir() {
		return true
	}

	// Check for Gemfile
	if info, err := os.Stat(filepath.Join(dir, "Gemfile")); err == nil && !info.IsDir() {
		return true
	}

	// Check for direnv
	direnvDir := os.Getenv("DIRENV_DIR")
	if direnvDir != "" && strings.TrimPrefix(direnvDir, "-") == dir {
		return true
	}

	return false
}

func shouldStop(dir string) bool {
	// Stop at root
	if dir == "/" || dir == "." {
		return true
	}

	// Stop at home directory
	if homeDir != "" && dir == homeDir {
		return true
	}

	return false
}

func findProjectRoot(pwd string) string {
	dir := pwd

	// If already in a project root, search parent projects
	if isProjectRoot(dir) {
		dir = filepath.Dir(dir)
	}

	for !shouldStop(dir) {
		if isProjectRoot(dir) {
			return dir
		}
		dir = filepath.Dir(dir)
	}

	return pwd
}

func abort(msg string) int {
	fmt.Fprintln(os.Stderr, msg)
	return 1
}
