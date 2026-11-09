/* SPDX-License-Identifier: MPL-2.0 */
/*
 * A headless CupertinoKit program in C: it negotiates the ABI version, starts a runtime without a display, receives
 * messages that another thread posts through a waker, shows the thread and handle rules, and releases everything.
 *
 * It prints one line per step on stdout, a trace that headless.expected holds, and exits 0. A step that does not give
 * the expected result prints why on stderr and exits 1.
 *
 * Build it against the installed SDK with the Makefile beside it (`make`), which takes every path from
 * `pkg-config cupertinokit`. It compiles as C11 and as C++17.
 */

#include <cupertinokit.h>

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* The two messages the waker thread posts. */
#define FIRST_MESSAGE 42
#define SECOND_MESSAGE 43
/* How long to wait for them: 5 s in steps of 1 ms. */
#define WAIT_STEPS 5000

static const char *status_name(ck_status status) {
  switch (status) {
  case CK_OK: return "CK_OK";
  case CK_ERROR_NULL_POINTER: return "CK_ERROR_NULL_POINTER";
  case CK_ERROR_INVALID_ARGUMENT: return "CK_ERROR_INVALID_ARGUMENT";
  case CK_ERROR_STALE_HANDLE: return "CK_ERROR_STALE_HANDLE";
  case CK_ERROR_WRONG_THREAD: return "CK_ERROR_WRONG_THREAD";
  case CK_ERROR_BUFFER_TOO_SMALL: return "CK_ERROR_BUFFER_TOO_SMALL";
  case CK_ERROR_VERSION: return "CK_ERROR_VERSION";
  case CK_ERROR_UNAVAILABLE: return "CK_ERROR_UNAVAILABLE";
  case CK_ERROR_UNSUPPORTED: return "CK_ERROR_UNSUPPORTED";
  case CK_ERROR_STATE: return "CK_ERROR_STATE";
  case CK_ERROR_QUEUE_FULL: return "CK_ERROR_QUEUE_FULL";
  case CK_ERROR_INTERNAL: return "CK_ERROR_INTERNAL";
  default: return "an unknown status";
  }
}

/* The calling thread's last error message, NUL-terminated. */
static const char *last_error(void) {
  static char message[512];
  size_t len = 0;
  if (ck_last_error_message((uint8_t *)message, sizeof message, &len) != CK_OK) {
    return "(no message)";
  }
  return message;
}

/* Prints `label: STATUS` and fails unless the call gave `expected`. */
static void expect(const char *label, ck_status status, ck_status expected) {
  if (status != expected) {
    fprintf(stderr, "%s: %s, expected %s: %s\n", label, status_name(status), status_name(expected), last_error());
    exit(1);
  }
  printf("%s: %s\n", label, status_name(status));
}

static void check(const char *label, int ok) {
  if (!ok) {
    fprintf(stderr, "%s: no\n", label);
    exit(1);
  }
  printf("%s: yes\n", label);
}

static CkStr str(const char *text) {
  CkStr s;
  s.ptr = (const uint8_t *)text;
  s.len = strlen(text);
  return s;
}

static CkRuntimeOptions options(uint32_t abi_minor) {
  CkRuntimeOptions o;
  memset(&o, 0, sizeof o);
  o.struct_size = sizeof o;
  o.abi_major = CUPERTINOKIT_ABI_MAJOR;
  o.abi_minor = abi_minor;
  o.backend = CK_BACKEND_HEADLESS;
  o.app_id = str("org.cupertino.examples.Headless");
  return o;
}

static void sleep_ms(long ms) {
  struct timespec t;
  t.tv_sec = ms / 1000;
  t.tv_nsec = (ms % 1000) * 1000000L;
  nanosleep(&t, NULL);
}

/* Posts both messages from a thread of its own: the waker is the one handle every thread may use. */
static void *post_messages(void *arg) {
  CkWaker waker = *(const CkWaker *)arg;
  ck_status *result = (ck_status *)malloc(sizeof(ck_status));
  *result = ck_waker_post(waker, FIRST_MESSAGE);
  if (*result == CK_OK) {
    *result = ck_waker_post(waker, SECOND_MESSAGE);
  }
  return result;
}

/* Polls the runtime from a thread that does not own it. */
static void *poll_elsewhere(void *arg) {
  CkRuntime runtime = *(const CkRuntime *)arg;
  CkEvent events[1];
  uint8_t bytes[64];
  size_t n_events = 0, n_bytes = 0;
  ck_status *result = (ck_status *)malloc(sizeof(ck_status));
  *result = ck_poll_events(runtime, events, 1, bytes, sizeof bytes, &n_events, &n_bytes);
  return result;
}

static ck_status join(pthread_t thread) {
  void *value = NULL;
  ck_status status;
  if (pthread_join(thread, &value) != 0 || value == NULL) {
    fprintf(stderr, "a thread could not be joined\n");
    exit(1);
  }
  status = *(ck_status *)value;
  free(value);
  return status;
}

int main(void) {
  uint32_t major = 0, minor = 0, has = 0;
  CkRuntimeOptions too_new;
  CkRuntimeOptions wanted = options(CUPERTINOKIT_ABI_MINOR);
  CkRuntime runtime, second;
  CkWaker waker;
  pthread_t thread;
  char needed[32];
  uint64_t received[2];
  size_t count = 0;
  int step;

  /* The library is at least as new as the header this program was built with. */
  expect("abi version", ck_abi_version(&major, &minor), CK_OK);
  check("same major version as the header", major == CUPERTINOKIT_ABI_MAJOR);
  /* + 1 on both sides: with header minor 0 the plain comparison is always true, which -Wextra reports */
  check("minor version not older than the header", minor + 1 >= CUPERTINOKIT_ABI_MINOR + 1u);

  /* Asking for a minor version the library does not have is refused, with a message that says why. */
  too_new = options(minor + 1);
  memset(&runtime, 0, sizeof runtime);
  expect("runtime asking for a newer minor version", ck_runtime_new(&too_new, &runtime), CK_ERROR_VERSION);
  snprintf(needed, sizeof needed, "%u.%u", (unsigned)CUPERTINOKIT_ABI_MAJOR, (unsigned)(minor + 1));
  check("the message names the version asked for", strstr(last_error(), needed) != NULL);

  /* Capabilities say what this library has before a function that may be missing is called. */
  expect("capability backend.headless", ck_has_capability(str("backend.headless"), &has), CK_OK);
  check("the headless backend is there", has == 1);
  expect("capability example.unknown", ck_has_capability(str("example.unknown"), &has), CK_OK);
  check("an unknown capability is not there", has == 0);

  /* One runtime per process, owned by the thread that made it. */
  expect("runtime", ck_runtime_new(&wanted, &runtime), CK_OK);
  expect("second runtime", ck_runtime_new(&wanted, &second), CK_ERROR_STATE);
  if (pthread_create(&thread, NULL, poll_elsewhere, &runtime) != 0) {
    fprintf(stderr, "no thread\n");
    return 1;
  }
  expect("poll from another thread", join(thread), CK_ERROR_WRONG_THREAD);

  /* A waker posts from any thread; the owner polls the messages in order. */
  expect("waker", ck_runtime_waker(runtime, &waker), CK_OK);
  if (pthread_create(&thread, NULL, post_messages, &waker) != 0) {
    fprintf(stderr, "no thread\n");
    return 1;
  }
  expect("posts from another thread", join(thread), CK_OK);
  for (step = 0; step < WAIT_STEPS && count < 2; step++) {
    CkEvent events[4];
    uint8_t bytes[64];
    size_t n_events = 0, n_bytes = 0, i;
    if (ck_poll_events(runtime, events, 4, bytes, sizeof bytes, &n_events, &n_bytes) != CK_OK) {
      fprintf(stderr, "poll: %s\n", last_error());
      return 1;
    }
    for (i = 0; i < n_events && count < 2; i++) {
      if (events[i].kind == CK_EVENT_MESSAGE) {
        received[count++] = events[i].message;
      }
    }
    if (count < 2) {
      sleep_ms(1);
    }
  }
  check("both messages arrived", count == 2);
  printf("message: %llu\n", (unsigned long long)received[0]);
  printf("message: %llu\n", (unsigned long long)received[1]);

  /* Released handles are stale, never dangling. */
  expect("release waker", ck_waker_release(waker), CK_OK);
  expect("post through the released waker", ck_waker_post(waker, FIRST_MESSAGE), CK_ERROR_STALE_HANDLE);
  expect("release runtime", ck_runtime_release(runtime), CK_OK);
  expect("waker of the released runtime", ck_runtime_waker(runtime, &waker), CK_ERROR_STALE_HANDLE);
  expect("release the runtime again", ck_runtime_release(runtime), CK_ERROR_STALE_HANDLE);
  printf("done\n");
  return 0;
}
