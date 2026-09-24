#ifndef UTIL_H
#define UTIL_H

#include <stddef.h>

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

// Write to out the absolute path of the program `name` installed next to this executable. The
// executable is found through /proc/self/exe on Linux and _NSGetExecutablePath on macOS, else
// argv0 (looked up in $PATH when it has no slash). Falls back to plain `name`.
void sibling_exe(char *out, size_t size, const char *argv0, const char *name);

#endif
