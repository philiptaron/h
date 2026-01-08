CC ?= cc
CFLAGS ?= -O2 -Wall
PKG_CONFIG ?= pkg-config

CURL_CFLAGS := $(shell $(PKG_CONFIG) --cflags libcurl libcjson)
CURL_LIBS := $(shell $(PKG_CONFIG) --libs libcurl libcjson)

PREFIX ?= /usr/local

all: h up

h: h.c
	$(CC) $(CFLAGS) $(CURL_CFLAGS) -o $@ $< $(CURL_LIBS)

up: up.c
	$(CC) $(CFLAGS) -o $@ $<

install: h up
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 h up $(DESTDIR)$(PREFIX)/bin

clean:
	rm -f h up

.PHONY: all install clean
