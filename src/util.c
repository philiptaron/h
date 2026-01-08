#define _DEFAULT_SOURCE
#include "util.h"
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

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
