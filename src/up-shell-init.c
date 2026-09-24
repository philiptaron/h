#define _DEFAULT_SOURCE
#include "util.h"
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
  const char *cd_cmd = "cd";

  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "--pushd") == 0) {
      cd_cmd = "pushd";
    } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
      printf("Usage: eval \"$(up-shell-init [--pushd])\"\n");
      return 0;
    } else {
      char msg[512];
      snprintf(msg, sizeof(msg), "Unknown option: %s", argv[i]);
      return fail(msg);
    }
  }

  char exe[PATH_MAX];
  sibling_exe(exe, sizeof(exe), argv[0], "up");

  printf("up() {\n"
         "  _up_dir=$(command %s \"$@\")\n"
         "  if [ $? = 0 ]; then\n"
         "    [ \"$_up_dir\" != \"$PWD\" ] && %s \"$_up_dir\"\n"
         "  fi\n"
         "}\n",
         exe,
         cd_cmd);

  return 0;
}
