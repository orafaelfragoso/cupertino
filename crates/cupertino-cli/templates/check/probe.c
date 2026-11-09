/* The probe `cupertino sdk check` compiles against the installed SDK and runs: the ABI version of the header and of
 * the library, then a headless runtime started and released. */
#include <cupertinokit.h>

#include <stdio.h>
#include <string.h>

int main(void) {
  uint32_t major = 0, minor = 0;
  CkRuntimeOptions options;
  CkRuntime runtime;
  ck_status status;

  printf("header %u.%u\n", (unsigned)CUPERTINOKIT_ABI_MAJOR, (unsigned)CUPERTINOKIT_ABI_MINOR);
  status = ck_abi_version(&major, &minor);
  if (status != CK_OK) {
    printf("abi_version %d\n", (int)status);
    return 1;
  }
  printf("library %u.%u\n", (unsigned)major, (unsigned)minor);
  memset(&options, 0, sizeof options);
  options.struct_size = sizeof options;
  options.abi_major = CUPERTINOKIT_ABI_MAJOR;
  options.abi_minor = CUPERTINOKIT_ABI_MINOR;
  options.backend = CK_BACKEND_HEADLESS;
  options.app_id.ptr = (const uint8_t *)"org.cupertino.SdkCheck";
  options.app_id.len = strlen("org.cupertino.SdkCheck");
  memset(&runtime, 0, sizeof runtime);
  status = ck_runtime_new(&options, &runtime);
  printf("runtime %d\n", (int)status);
  if (status == CK_OK) {
    status = ck_runtime_release(runtime);
    printf("release %d\n", (int)status);
  }
  return status == CK_OK ? 0 : 1;
}
