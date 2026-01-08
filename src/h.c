#define _DEFAULT_SOURCE
#include "util.h"
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cjson/cJSON.h>
#include <curl/curl.h>

#define cleanup(func) __attribute__((cleanup(func)))

static void free_char(char **p) {
  free(*p);
}
static void free_curl(CURL **p) {
  if (*p)
    curl_easy_cleanup(*p);
}
static void free_slist(struct curl_slist **p) {
  if (*p)
    curl_slist_free_all(*p);
}
static void free_cjson(cJSON **p) {
  if (*p)
    cJSON_Delete(*p);
}
static void close_dir(DIR **p) {
  if (*p)
    closedir(*p);
}
static void curl_cleanup(char *p) {
  (void)p;
  curl_global_cleanup();
}

typedef struct {
  char *data;
  size_t size;
} Buffer;

static void free_buffer(Buffer *p) {
  free(p->data);
}

static size_t write_callback(void *contents, size_t size, size_t nmemb, void *userp) {
  size_t total = size * nmemb;
  Buffer *buf = (Buffer *)userp;
  char *ptr = realloc(buf->data, buf->size + total + 1);
  if (!ptr)
    return 0;
  buf->data = ptr;
  memcpy(&buf->data[buf->size], contents, total);
  buf->size += total;
  buf->data[buf->size] = '\0';
  return total;
}

static int fetch_github_repo_info(const char *user,
                                  const char *repo,
                                  char *out_owner,
                                  size_t owner_size,
                                  char *out_repo,
                                  size_t repo_size) {
  char url[512];
  snprintf(url, sizeof(url), "https://api.github.com/repos/%s/%s", user, repo);

  cleanup(free_curl) CURL *curl = curl_easy_init();
  if (!curl)
    return 0;

  cleanup(free_buffer) Buffer buf = {0};
  cleanup(free_slist) struct curl_slist *headers = NULL;
  headers = curl_slist_append(headers, "User-Agent: h-cli");
  headers = curl_slist_append(headers, "Accept: application/vnd.github.v3+json");

  curl_easy_setopt(curl, CURLOPT_URL, url);
  curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
  curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_callback);
  curl_easy_setopt(curl, CURLOPT_WRITEDATA, &buf);
  curl_easy_setopt(curl, CURLOPT_TIMEOUT, 10L);

  CURLcode res = curl_easy_perform(curl);
  long http_code = 0;
  curl_easy_getinfo(curl, CURLINFO_RESPONSE_CODE, &http_code);

  if (res != CURLE_OK || http_code != 200)
    return 0;

  cleanup(free_cjson) cJSON *json = cJSON_Parse(buf.data);
  if (!json)
    return 0;

  cJSON *owner_obj = cJSON_GetObjectItem(json, "owner");
  cJSON *name_obj = cJSON_GetObjectItem(json, "name");

  if (owner_obj && name_obj) {
    cJSON *login = cJSON_GetObjectItem(owner_obj, "login");
    if (login && cJSON_IsString(login) && cJSON_IsString(name_obj)) {
      strncpy(out_owner, login->valuestring, owner_size - 1);
      out_owner[owner_size - 1] = '\0';
      strncpy(out_repo, name_obj->valuestring, repo_size - 1);
      out_repo[repo_size - 1] = '\0';
      return 1;
    }
  }

  return 0;
}

static int is_valid_name_char(char c) {
  return isalnum(c) || c == '.' || c == '-' || c == '_';
}

static int is_simple_name(const char *s) {
  if (!*s)
    return 0;
  for (; *s; s++)
    if (!is_valid_name_char(*s))
      return 0;
  return 1;
}

static int is_github_repo(const char *host,
                          const char *path,
                          char *user,
                          size_t user_size,
                          char *repo,
                          size_t repo_size) {
  if (strcmp(host, "github.com") != 0)
    return 0;

  const char *slash = strchr(path, '/');
  if (!slash || slash == path || !slash[1])
    return 0;
  if (strchr(slash + 1, '/'))
    return 0;

  size_t ulen = slash - path;
  size_t rlen = strlen(slash + 1);
  for (size_t i = 0; i < ulen; i++)
    if (!is_valid_name_char(path[i]))
      return 0;
  for (size_t i = 0; i < rlen; i++)
    if (!is_valid_name_char(slash[1 + i]))
      return 0;

  strncpy(user, path, ulen < user_size ? ulen : user_size - 1);
  user[ulen < user_size ? ulen : user_size - 1] = '\0';
  strncpy(repo, slash + 1, repo_size - 1);
  repo[repo_size - 1] = '\0';
  return 1;
}

static void correct_github_casing(char *user, size_t user_size, char *repo, size_t repo_size) {
  char api_owner[256], api_repo[256];
  if (fetch_github_repo_info(user,
                             repo,
                             api_owner,
                             sizeof(api_owner),
                             api_repo,
                             sizeof(api_repo))) {
    strncpy(user, api_owner, user_size - 1);
    user[user_size - 1] = '\0';
    strncpy(repo, api_repo, repo_size - 1);
    repo[repo_size - 1] = '\0';
  }
}

static void setup_github_clone(const char *code_root,
                               char *user,
                               size_t user_size,
                               char *repo,
                               size_t repo_size,
                               char *url,
                               size_t url_size,
                               char *path,
                               size_t path_size) {
  correct_github_casing(user, user_size, repo, repo_size);
  snprintf(url, url_size, "https://github.com/%s/%s.git", user, repo);
  snprintf(path, path_size, "%s/github.com/%s/%s", code_root, user, repo);
}

typedef struct {
  char path[PATH_MAX];
  int depth;
} SearchResult;

static void search_dir(const char *dir,
                       const char *term,
                       int case_sensitive,
                       int depth,
                       int max_depth,
                       SearchResult *best) {
  if (depth > max_depth)
    return;

  cleanup(close_dir) DIR *d = opendir(dir);
  if (!d)
    return;

  struct dirent *ent;
  while ((ent = readdir(d))) {
    if (ent->d_name[0] == '.')
      continue;

    char path[PATH_MAX];
    snprintf(path, sizeof(path), "%s/%s", dir, ent->d_name);

    if (!is_dir(path))
      continue;

    int match;
    if (case_sensitive) {
      match = strcmp(ent->d_name, term) == 0;
    } else {
      match = strcasecmp(ent->d_name, term) == 0;
    }

    if (match && depth > best->depth) {
      strncpy(best->path, path, sizeof(best->path) - 1);
      best->depth = depth;
    }

    search_dir(path, term, case_sensitive, depth + 1, max_depth, best);
  }
}

static void mkpath(const char *path) {
  char tmp[PATH_MAX];
  strncpy(tmp, path, sizeof(tmp) - 1);
  for (char *p = tmp + 1; *p; p++) {
    if (*p == '/') {
      *p = '\0';
      mkdir(tmp, 0755);
      *p = '/';
    }
  }
  mkdir(tmp, 0755);
}

static int clone_repo(const char *url, const char *path, int argc, char **argv) {
  char parent[PATH_MAX];
  strncpy(parent, path, sizeof(parent) - 1);
  char *last_slash = strrchr(parent, '/');
  if (last_slash)
    *last_slash = '\0';
  mkpath(parent);

  pid_t pid = fork();
  if (pid == 0) {
    char **args = malloc((argc + 6) * sizeof(char *));
    int i = 0;
    args[i++] = "git";
    args[i++] = "clone";
    if (argc == 0)
      args[i++] = "--recursive";
    for (int j = 0; j < argc; j++)
      args[i++] = argv[j];
    args[i++] = "--";
    args[i++] = (char *)url;
    args[i++] = (char *)path;
    args[i] = NULL;

    dup2(STDERR_FILENO, STDOUT_FILENO);
    execvp("git", args);
    _exit(127);
  }

  int status;
  waitpid(pid, &status, 0);
  return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}

static void strip_git_extension(char *path) {
  size_t len = strlen(path);
  if (len > 4 && strcmp(path + len - 4, ".git") == 0)
    path[len - 4] = '\0';
}

int main(int argc, char **argv) {
  if (argc < 2)
    return fail_with_cwd("Usage: eval \"$(h-shell-init [options] [code-root])\"");

  if (strcmp(argv[1], "--resolve") != 0)
    return fail_with_cwd("h is not installed\n\nUsage: eval \"$(h-shell-init [code-root])\"");

  if (argc < 3)
    return fail_with_cwd("Usage: h --resolve <code-root> <term>");

  cleanup(free_char) char *code_root = expand_tilde(argv[2]);
  if (argc < 4)
    return fail_with_cwd("Usage: h (<name> | <repo>/<name> | <url>) [git opts]");

  const char *term = argv[3];
  if (strcmp(term, "-h") == 0 || strcmp(term, "--help") == 0)
    return fail_with_cwd("Usage: h (<name> | <repo>/<name> | <url>) [git opts]");

  curl_global_init(CURL_GLOBAL_DEFAULT);
  cleanup(curl_cleanup) char curl_guard = 0;

  char path[PATH_MAX] = {0};
  char url[PATH_MAX] = {0};
  char user[256], repo[256];

  if (is_github_repo("github.com", term, user, sizeof(user), repo, sizeof(repo))) {
    setup_github_clone(code_root,
                       user,
                       sizeof(user),
                       repo,
                       sizeof(repo),
                       url,
                       sizeof(url),
                       path,
                       sizeof(path));
  } else if (strstr(term, "://")) {
    char host[256] = {0}, uri_path[PATH_MAX] = {0};
    const char *p = strstr(term, "://");
    if (!p)
      return fail_with_cwd("Missing url scheme");
    p += 3;
    const char *slash = strchr(p, '/');
    if (slash) {
      size_t hlen = slash - p;
      strncpy(host, p, hlen < sizeof(host) ? hlen : sizeof(host) - 1);
      strncpy(uri_path, slash + 1, sizeof(uri_path) - 1);
    } else {
      strncpy(host, p, sizeof(host) - 1);
    }
    for (char *c = host; *c; c++)
      *c = tolower(*c);
    if (is_github_repo(host, uri_path, user, sizeof(user), repo, sizeof(repo))) {
      setup_github_clone(code_root,
                         user,
                         sizeof(user),
                         repo,
                         sizeof(repo),
                         url,
                         sizeof(url),
                         path,
                         sizeof(path));
    } else {
      strncpy(url, term, sizeof(url) - 1);
      snprintf(path, sizeof(path), "%s/%s/%s", code_root, host, uri_path);
    }
  } else if (strncmp(term, "git@", 4) == 0 || strncmp(term, "gitea@", 6) == 0) {
    const char *at = strchr(term, '@');
    const char *colon = strchr(at, ':');
    if (at && colon) {
      char host[256] = {0};
      size_t hlen = colon - at - 1;
      strncpy(host, at + 1, hlen < sizeof(host) ? hlen : sizeof(host) - 1);
      for (char *c = host; *c; c++)
        *c = tolower(*c);
      const char *repo_path = colon + 1;
      if (is_github_repo(host, repo_path, user, sizeof(user), repo, sizeof(repo))) {
        setup_github_clone(code_root,
                           user,
                           sizeof(user),
                           repo,
                           sizeof(repo),
                           url,
                           sizeof(url),
                           path,
                           sizeof(path));
      } else {
        strncpy(url, term, sizeof(url) - 1);
        snprintf(path, sizeof(path), "%s/%s/%s", code_root, host, repo_path);
      }
    }
  } else if (is_simple_name(term)) {
    int case_sensitive = 0;
    for (const char *c = term; *c; c++) {
      if (isupper(*c)) {
        case_sensitive = 1;
        break;
      }
    }
    SearchResult best = {.depth = 0};
    search_dir(code_root, term, case_sensitive, 1, 3, &best);
    if (best.depth > 0) {
      strncpy(path, best.path, sizeof(path) - 1);
    }
  } else {
    char msg[512];
    snprintf(msg, sizeof(msg), "Unknown pattern for %s", term);
    return fail_with_cwd(msg);
  }

  if (!path[0]) {
    char msg[512];
    snprintf(msg, sizeof(msg), "%s not found", term);
    return fail_with_cwd(msg);
  }

  strip_git_extension(path);

  if (is_dir(path)) {
    puts(path);
    return 0;
  }

  if (!url[0]) {
    char msg[512];
    snprintf(msg, sizeof(msg), "%s not found", term);
    return fail_with_cwd(msg);
  }

  int ret = clone_repo(url, path, argc - 4, argv + 4);
  if (ret != 0) {
    char cwd[PATH_MAX];
    if (getcwd(cwd, sizeof(cwd)))
      puts(cwd);
    return ret;
  }

  puts(path);
  return 0;
}
