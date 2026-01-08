CFLAGS += $(shell pkg-config --cflags libcurl libcjson)
LDFLAGS += $(shell pkg-config --libs libcurl libcjson)

all: h up h-shell-init up-shell-init

h: h.c
	$(CC) $(CFLAGS) -o $@ $< $(LDFLAGS)

up: up.c
	$(CC) $(CFLAGS) -o $@ $<

h-shell-init: h-shell-init.c
	$(CC) $(CFLAGS) -o $@ $<

up-shell-init: up-shell-init.c
	$(CC) $(CFLAGS) -o $@ $<

install:
	install -Dm755 h up h-shell-init up-shell-init -t $(PREFIX)/bin
