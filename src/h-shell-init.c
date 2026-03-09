#define _DEFAULT_SOURCE
#include "util.h"
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

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
           "  _h_term=\"$1\"\n"
           "  shift\n"
           "  _h_dir=$(command %s --resolve \"%s\" \"$_h_term\" %s \"$@\")\n"
           "  _h_ret=$?\n"
           "  [ \"$_h_dir\" != \"$PWD\" ] && %s \"$_h_dir\"\n"
           "  return $_h_ret\n"
           "}\n",
           func_name,
           exe,
           code_root,
           git_opts,
           cd_cmd);
  } else {
    printf("%s() {\n"
           "  _h_dir=$(command %s --resolve \"%s\" \"$@\")\n"
           "  _h_ret=$?\n"
           "  [ \"$_h_dir\" != \"$PWD\" ] && %s \"$_h_dir\"\n"
           "  return $_h_ret\n"
           "}\n",
           func_name,
           exe,
           code_root,
           cd_cmd);
  }

  // Output shell-appropriate tab completion
  printf("if [ -n \"$ZSH_VERSION\" ]; then\n"
         "  _%s_complete() {\n"
         "    local code_root='%s'\n"
         "    local -a projects\n"
         "    [[ -d \"$code_root\" ]] || return\n"
         "    projects=(\n"
         "      \"$code_root\"/*(N/:t)\n"
         "      \"$code_root\"/*/*(N/:t)\n"
         "      \"$code_root\"/*/*/*(N/:t)\n"
         "    )\n"
         "    projects=(\"${(u)projects[@]}\")\n"
         "    compadd -a projects\n"
         "  }\n"
         "  compdef _%s_complete %s\n"
         "elif [ -n \"$BASH_VERSION\" ]; then\n"
         "  _%s_complete() {\n"
         "    local cur=\"${COMP_WORDS[COMP_CWORD]}\"\n"
         "    local code_root='%s'\n"
         "    COMPREPLY=()\n"
         "    [[ -d \"$code_root\" ]] || return\n"
         "    local dirs\n"
         "    dirs=$(find \"$code_root\" -mindepth 1 -maxdepth 3 -type d "
         "-not -name '.*' 2>/dev/null | sed 's|.*/||' | sort -u)\n"
         "    COMPREPLY=($(compgen -W \"$dirs\" -- \"$cur\"))\n"
         "  }\n"
         "  complete -F _%s_complete %s\n"
         "fi\n",
         func_name,
         code_root,
         func_name,
         func_name,
         func_name,
         code_root,
         func_name,
         func_name);

  free(code_root);
  return 0;
}
