#define _DEFAULT_SOURCE
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int is_dir(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

static int is_file(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode);
}

static int is_root(const char *path) {
    return strcmp(path, "/") == 0;
}

static int is_home(const char *path) {
    const char *home = getenv("HOME");
    return home && strcmp(path, home) == 0;
}

static int is_project_root(const char *dir) {
    char path[PATH_MAX];

    snprintf(path, sizeof(path), "%s/.git", dir);
    if (is_dir(path)) return 1;

    snprintf(path, sizeof(path), "%s/.hg", dir);
    if (is_dir(path)) return 1;

    snprintf(path, sizeof(path), "%s/.envrc", dir);
    if (is_file(path)) return 1;

    snprintf(path, sizeof(path), "%s/Gemfile", dir);
    if (is_file(path)) return 1;

    const char *direnv_dir = getenv("DIRENV_DIR");
    if (direnv_dir && direnv_dir[0] == '-' && strcmp(dir, direnv_dir + 1) == 0)
        return 1;

    return 0;
}

static void parent_dir(char *path) {
    char *last_slash = strrchr(path, '/');
    if (last_slash && last_slash != path) {
        *last_slash = '\0';
    } else if (last_slash == path) {
        path[1] = '\0';
    }
}

int main(int argc, char **argv) {
    if (argc > 1) {
        if (strcmp(argv[1], "--setup") == 0) {
            printf("up() {\n"
                   "  _up_dir=$(command up \"$@\")\n"
                   "  if [ $? = 0 ]; then\n"
                   "    [ \"$_up_dir\" != \"$PWD\" ] && cd \"$_up_dir\"\n"
                   "  fi\n"
                   "}\n");
            return 0;
        }
        if (strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
            char cwd[PATH_MAX];
            if (getcwd(cwd, sizeof(cwd))) puts(cwd);
            fprintf(stderr, "up is not installed\n\n"
                           "Usage: eval \"$(up --setup)\"\n");
            return 1;
        }
    }

    char cwd[PATH_MAX];
    const char *pwd = getenv("PWD");
    if (pwd) {
        strncpy(cwd, pwd, sizeof(cwd) - 1);
        cwd[sizeof(cwd) - 1] = '\0';
    } else if (!getcwd(cwd, sizeof(cwd))) {
        perror("getcwd");
        return 1;
    }

    char dir[PATH_MAX];
    strncpy(dir, cwd, sizeof(dir) - 1);
    dir[sizeof(dir) - 1] = '\0';

    if (is_project_root(dir)) {
        parent_dir(dir);
    }

    while (!is_root(dir) && !is_home(dir)) {
        if (is_project_root(dir)) {
            puts(dir);
            return 0;
        }
        parent_dir(dir);
    }

    puts(cwd);
    return 0;
}
