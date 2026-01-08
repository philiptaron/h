#define _DEFAULT_SOURCE
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static char *expand_tilde(const char *path) {
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

static int fail(const char *msg) {
  if (msg)
    fprintf(stderr, "%s\n", msg);
  return 1;
}

int main(int argc, char **argv) {
  const char *func_name = "h";
  const char *cd_cmd = "cd";
  const char *git_opts = "";
  const char *code_root_arg = NULL;

  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "--pushd") == 0) {
      cd_cmd = "pushd";
    } else if (strcmp(argv[i], "--name") == 0 && i + 1 < argc) {
      func_name = argv[++i];
    } else if (strcmp(argv[i], "--git-opts") == 0 && i + 1 < argc) {
      git_opts = argv[++i];
    } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
      printf("Usage: eval \"$(h-shell-init [--pushd] [--name NAME] "
             "[--git-opts \"OPTIONS\"] [code-root])\"\n");
      return 0;
    } else if (argv[i][0] != '-') {
      code_root_arg = argv[i];
    } else {
      char msg[512];
      snprintf(msg, sizeof(msg), "Unknown option: %s", argv[i]);
      return fail(msg);
    }
  }

  const char *default_root = getenv("H_CODE_ROOT");
  if (!default_root)
    default_root = "~/src";
  if (!code_root_arg)
    code_root_arg = default_root;
  char *code_root = expand_tilde(code_root_arg);

  char exe[PATH_MAX];
  ssize_t len = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
  if (len == -1) {
    char *pwd = getenv("PWD");
    if (!pwd)
      pwd = ".";
    if (argv[0][0] == '/')
      strncpy(exe, argv[0], sizeof(exe) - 1);
    else
      snprintf(exe, sizeof(exe), "%s/%s", pwd, argv[0]);
  } else {
    exe[len] = '\0';
  }

  // Replace "h-shell-init" with "h" in the path
  char *basename = strrchr(exe, '/');
  if (basename) {
    snprintf(basename + 1, sizeof(exe) - (basename - exe) - 1, "h");
  } else {
    strncpy(exe, "h", sizeof(exe) - 1);
  }

  if (git_opts[0]) {
    printf("%s() {\n"
           "  _h_dir=$(command %s --resolve \"%s\" %s \"$@\")\n"
           "  _h_ret=$?\n"
           "  [ \"$_h_dir\" != \"$PWD\" ] && %s \"$_h_dir\"\n"
           "  return $_h_ret\n"
           "}\n",
           func_name, exe, code_root, git_opts, cd_cmd);
  } else {
    printf("%s() {\n"
           "  _h_dir=$(command %s --resolve \"%s\" \"$@\")\n"
           "  _h_ret=$?\n"
           "  [ \"$_h_dir\" != \"$PWD\" ] && %s \"$_h_dir\"\n"
           "  return $_h_ret\n"
           "}\n",
           func_name, exe, code_root, cd_cmd);
  }

  free(code_root);
  return 0;
}
