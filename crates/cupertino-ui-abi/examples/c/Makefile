# Builds the C samples against the installed CupertinoKit SDK (the cupertino-kit package). Every include and library
# path comes from `pkg-config cupertinokit`; set PKG_CONFIG_SYSROOT_DIR and PKG_CONFIG_LIBDIR to build against an SDK
# installed elsewhere, and LD_LIBRARY_PATH to run the result there.
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

SAMPLES = headless

all: $(SAMPLES)

clean:
	rm -f $(SAMPLES)

.PHONY: all clean
