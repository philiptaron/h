CFLAGS += $(shell pkg-config --cflags libcurl libcjson)
LDFLAGS += $(shell pkg-config --libs libcurl libcjson)

all: h up h-shell-init up-shell-init

util.o: util.c util.h
	$(CC) $(CFLAGS) -c -o $@ $<

h: h.c util.o
	$(CC) $(CFLAGS) -o $@ h.c util.o $(LDFLAGS)

up: up.c util.o
	$(CC) $(CFLAGS) -o $@ up.c util.o

h-shell-init: h-shell-init.c util.o
	$(CC) $(CFLAGS) -o $@ h-shell-init.c util.o

up-shell-init: up-shell-init.c util.o
	$(CC) $(CFLAGS) -o $@ up-shell-init.c util.o

install:
	install -Dm755 h up h-shell-init up-shell-init -t $(PREFIX)/bin
