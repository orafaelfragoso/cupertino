/* {{name}}: a CupertinoKit application in C.
 *
 * It starts a runtime, lets a second thread post three ticks to it through a waker, counts them on the runtime's own
 * thread and exits. The C ABI's rules are in the header, /usr/include/cupertinokit-1/cupertinokit.h. */
#include <cupertinokit.h>

#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

#define APP_ID "{{app_id}}"
#define TICKS 3

static void sleep_ms(long ms) {
  struct timespec t;
  t.tv_sec = ms / 1000;
  t.tv_nsec = (ms % 1000) * 1000000L;
  nanosleep(&t, NULL);
}

static const char *last_error(void) {
  static char message[512];
  size_t len = 0;
  if (ck_last_error_message((uint8_t *)message, sizeof message, &len) != CK_OK) {
    return "(no message)";
  }
  return message;
}

/* Wakers are the one handle any thread may use. */
static void *tick(void *arg) {
  CkWaker waker = *(const CkWaker *)arg;
  uint64_t i;
  for (i = 1; i <= TICKS; i++) {
    sleep_ms(100);
    ck_waker_post(waker, i);
  }
  return NULL;
}

int main(void) {
  CkRuntimeOptions options;
  CkRuntime runtime;
  CkWaker waker;
  pthread_t thread;
  int count = 0, step;

  memset(&options, 0, sizeof options);
  options.struct_size = sizeof options;
  options.abi_major = CUPERTINOKIT_ABI_MAJOR;
  options.abi_minor = CUPERTINOKIT_ABI_MINOR;
  /* ABI 1.0 has the headless backend only */
  options.backend = CK_BACKEND_HEADLESS;
  options.app_id.ptr = (const uint8_t *)APP_ID;
  options.app_id.len = strlen(APP_ID);
  if (ck_runtime_new(&options, &runtime) != CK_OK) {
    fprintf(stderr, "%s: %s\n", APP_ID, last_error());
    return 1;
  }
  if (ck_runtime_waker(runtime, &waker) != CK_OK || pthread_create(&thread, NULL, tick, &waker) != 0) {
    fprintf(stderr, "%s: no waker thread: %s\n", APP_ID, last_error());
    ck_runtime_release(runtime);
    return 1;
  }
  /* poll the runtime in steps of 10 ms, for 5 s at most */
  for (step = 0; step < 500 && count < TICKS; step++) {
    CkEvent events[8];
    uint8_t bytes[256];
    size_t n_events = 0, n_bytes = 0, i;
    if (ck_poll_events(runtime, events, 8, bytes, sizeof bytes, &n_events, &n_bytes) != CK_OK) {
      fprintf(stderr, "%s: %s\n", APP_ID, last_error());
      break;
    }
    for (i = 0; i < n_events; i++) {
      if (events[i].kind == CK_EVENT_MESSAGE) {
        printf("count: %d\n", ++count);
      }
    }
    sleep_ms(10);
  }
  pthread_join(thread, NULL);
  ck_waker_release(waker);
  ck_runtime_release(runtime);
  return count == TICKS ? 0 : 1;
}
