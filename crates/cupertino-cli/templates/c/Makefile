# Builds {{name}} against the installed CupertinoKit SDK (the cupertino-kit package). Every include and library
# path comes from `pkg-config cupertinokit`.
PKG_CONFIG ?= pkg-config
CFLAGS ?= -O2 -g
override CFLAGS += -std=c11 -Wall -Wextra -pthread

ifneq ($(MAKECMDGOALS),clean)
ifneq ($(shell $(PKG_CONFIG) --exists cupertinokit && echo yes),yes)
$(error pkg-config finds no cupertinokit: install the cupertino-kit package)
endif
endif

override CFLAGS += $(shell $(PKG_CONFIG) --cflags cupertinokit)
override LDLIBS += $(shell $(PKG_CONFIG) --libs cupertinokit)

{{name}}: main.c
	$(CC) $(CPPFLAGS) $(CFLAGS) $(LDFLAGS) -o $@ main.c $(LDLIBS)

run: {{name}}
	./{{name}}

clean:
	rm -f {{name}}

.PHONY: run clean
