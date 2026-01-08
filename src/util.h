#ifndef UTIL_H
#define UTIL_H

// Expand leading ~ to $HOME. Returns allocated string.
char *expand_tilde(const char *path);

// Print error message to stderr, return 1.
int fail(const char *msg);

// Print cwd to stdout, error to stderr, return 1. For shell function fallback.
int fail_with_cwd(const char *msg);

// Check if path is a directory.
int is_dir(const char *path);

// Check if path is a regular file.
int is_file(const char *path);

#endif
