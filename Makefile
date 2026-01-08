CFLAGS += $(shell pkg-config --cflags libcurl libcjson)
LDFLAGS += $(shell pkg-config --libs libcurl libcjson)

all: h up

h: h.c
	$(CC) $(CFLAGS) -o $@ $< $(LDFLAGS)

up: up.c
	$(CC) $(CFLAGS) -o $@ $<

install:
	install -Dm755 h up -t $(PREFIX)/bin
