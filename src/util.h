#ifndef UTIL_H
#define UTIL_H

char *expand_tilde(const char *path);
int fail(const char *msg);
int fail_with_cwd(const char *msg);

#endif
