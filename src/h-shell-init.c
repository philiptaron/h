#define _DEFAULT_SOURCE
#include "util.h"
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifdef __APPLE__
#include <libproc.h>
#endif

// The parent process's command name (the shell running the eval), or "" if it cannot be found:
// proc_name on macOS, which has no /proc, and /proc/<ppid>/comm elsewhere.
static void parent_name(char *out, size_t size) {
  out[0] = '\0';
#ifdef __APPLE__
  int len = proc_name(getppid(), out, size);
  out[len > 0 && (size_t)len < size ? len : 0] = '\0';
#else
  char proc_path[64];
  snprintf(proc_path, sizeof(proc_path), "/proc/%d/comm", getppid());
  FILE *f = fopen(proc_path, "r");
  if (!f)
    return;
  if (fgets(out, size, f))
    out[strcspn(out, "\n")] = '\0';
  else
    out[0] = '\0';
  fclose(f);
#endif
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
  sibling_exe(exe, sizeof(exe), argv[0], "h");

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

  // Detect parent shell to emit only compatible completion code.
  // Emitting both zsh and bash branches in a single if/elif/fi doesn't work
  // because bash parses zsh glob qualifiers like *(N/:t) as syntax errors
  // even inside an untaken branch.
  enum { SHELL_UNKNOWN, SHELL_BASH, SHELL_ZSH } shell = SHELL_UNKNOWN;
  char parent_comm[256];
  parent_name(parent_comm, sizeof(parent_comm));
  if (strcmp(parent_comm, "zsh") == 0)
    shell = SHELL_ZSH;
  else if (strcmp(parent_comm, "bash") == 0)
    shell = SHELL_BASH;

  // Output tab completion for the detected shell
  if (shell == SHELL_ZSH) {
    printf("_%s_complete() {\n"
           "  local code_root='%s'\n"
           "  local -a projects\n"
           "  [[ -d \"$code_root\" ]] || return\n"
           "  projects=(\n"
           "    \"$code_root\"/*(N/:t)\n"
           "    \"$code_root\"/*/*(N/:t)\n"
           "    \"$code_root\"/*/*/*(N/:t)\n"
           "  )\n"
           "  projects=(\"${(u)projects[@]}\")\n"
           "  compadd -a projects\n"
           "}\n"
           "compdef _%s_complete %s\n",
           func_name,
           code_root,
           func_name,
           func_name);
  } else if (shell == SHELL_BASH) {
    printf("_%s_complete() {\n"
           "  local cur=\"${COMP_WORDS[COMP_CWORD]}\"\n"
           "  local code_root='%s'\n"
           "  COMPREPLY=()\n"
           "  [[ -d \"$code_root\" ]] || return\n"
           "  local dirs\n"
           "  dirs=$(find \"$code_root\" -mindepth 1 -maxdepth 3 -type d "
           "-not -name '.*' 2>/dev/null | sed 's|.*/||' | sort -u)\n"
           "  COMPREPLY=($(compgen -W \"$dirs\" -- \"$cur\"))\n"
           "}\n"
           "complete -F _%s_complete %s\n",
           func_name,
           code_root,
           func_name,
           func_name);
  }

  free(code_root);
  return 0;
}
