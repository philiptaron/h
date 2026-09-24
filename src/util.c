#define _DEFAULT_SOURCE
#include "util.h"
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifdef __APPLE__
#include <mach-o/dyld.h>
#include <stdint.h>
#endif

char *expand_tilde(const char *path) {
  if (path[0] != '~')
    return strdup(path);
  const char *home = getenv("HOME");
  if (!home)
    return strdup(path);
  size_t len = strlen(home) + strlen(path);
  char *result = malloc(len);
  snprintf(result, len, "%s%s", home, path + 1);
  return result;
}

int fail(const char *msg) {
  if (msg)
    fprintf(stderr, "%s\n", msg);
  return 1;
}

int fail_with_cwd(const char *msg) {
  char cwd[PATH_MAX];
  if (getcwd(cwd, sizeof(cwd)))
    puts(cwd);
  if (msg)
    fprintf(stderr, "%s\n", msg);
  return 1;
}

int is_dir(const char *path) {
  struct stat st;
  return stat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

int is_file(const char *path) {
  struct stat st;
  return stat(path, &st) == 0 && S_ISREG(st.st_mode);
}

// Prefix a relative path with the current directory.
static void make_absolute(char *path, size_t size) {
  char cwd[PATH_MAX];
  if (path[0] == '/' || !getcwd(cwd, sizeof(cwd)))
    return;
  char rel[PATH_MAX];
  strncpy(rel, path, sizeof(rel) - 1);
  rel[sizeof(rel) - 1] = '\0';
  snprintf(path, size, "%s/%s", cwd, rel);
}

// Find argv0 in $PATH the way the shell did. Returns 1 on success.
static int search_path(char *out, size_t size, const char *argv0) {
  const char *path = getenv("PATH");
  if (!path)
    return 0;
  while (*path) {
    const char *end = strchr(path, ':');
    size_t len = end ? (size_t)(end - path) : strlen(path);
    // an empty $PATH entry means the current directory
    if (len == 0)
      snprintf(out, size, "./%s", argv0);
    else
      snprintf(out, size, "%.*s/%s", (int)len, path, argv0);
    if (is_file(out) && access(out, X_OK) == 0)
      return 1;
    if (!end)
      break;
    path = end + 1;
  }
  return 0;
}

static int self_exe(char *exe, size_t size, const char *argv0) {
#ifdef __APPLE__
  // unlike /proc/self/exe this is the path it was started by, symlinks and all
  char path[PATH_MAX], real[PATH_MAX];
  uint32_t n = sizeof(path);
  if (_NSGetExecutablePath(path, &n) == 0 && realpath(path, real)) {
    snprintf(exe, size, "%s", real);
    return 1;
  }
#else
  ssize_t len = readlink("/proc/self/exe", exe, size - 1);
  if (len != -1) {
    exe[len] = '\0';
    return 1;
  }
#endif
  if (strchr(argv0, '/')) {
    snprintf(exe, size, "%s", argv0);
    return 1;
  }
  return search_path(exe, size, argv0);
}

void sibling_exe(char *out, size_t size, const char *argv0, const char *name) {
  if (!self_exe(out, size, argv0)) {
    snprintf(out, size, "%s", name);
    return;
  }
  make_absolute(out, size);
  char *slash = strrchr(out, '/');
  snprintf(slash + 1, size - (slash + 1 - out), "%s", name);
}
