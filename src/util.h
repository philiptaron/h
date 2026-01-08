#ifndef UTIL_H
#define UTIL_H

char *expand_tilde(const char *path);
int fail(const char *msg);
int fail_with_cwd(const char *msg);
int is_dir(const char *path);
int is_file(const char *path);

#endif
