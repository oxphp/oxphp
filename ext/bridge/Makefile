CC ?= gcc
CFLAGS ?= -O2 -Wall -Wextra -fPIC -D_GNU_SOURCE
PREFIX ?= /usr/local

# PHP include paths (needed for native zval accessor functions).
# php-config must be on $PATH; empty on host-only builds without PHP.
PHP_INCLUDES := $(shell php-config --includes 2>/dev/null)
CFLAGS += $(PHP_INCLUDES)

LIBNAME = liboxphp_bridge.so
OBJECTS = oxphp_bridge.o

.PHONY: all clean install

all: $(LIBNAME)

$(LIBNAME): $(OBJECTS)
	$(CC) -shared -o $@ $^

oxphp_bridge.o: oxphp_bridge.c oxphp_bridge.h
	$(CC) $(CFLAGS) -c -o $@ oxphp_bridge.c

install: $(LIBNAME)
	install -d $(DESTDIR)$(PREFIX)/lib
	install -m 755 $(LIBNAME) $(DESTDIR)$(PREFIX)/lib/
	install -d $(DESTDIR)$(PREFIX)/include
	install -m 644 oxphp_bridge.h $(DESTDIR)$(PREFIX)/include/

clean:
	rm -f $(OBJECTS) $(LIBNAME)
