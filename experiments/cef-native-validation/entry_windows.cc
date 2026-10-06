// Synthetic feasibility experiment; bootstrap ABI follows CEF 14c5a089
// tests/ceftests/run_all_unittests.cc and tests/shared/browser/util_win.cc.
#include <cstdio>
#include <fcntl.h>
#include <initializer_list>
#include <io.h>
#include <windows.h>

#include "host.h"
#include "include/cef_sandbox_win.h"

#if !defined(CEF_USE_BOOTSTRAP)
#error "The Windows probe requires the CEF console bootstrap and its sandbox."
#endif

// cef_sandbox_win.h supplies extern "C" and __declspec(dllexport). Like the
// pinned console sample, the client does not populate bootstrap version_info.
CEF_BOOTSTRAP_EXPORT int RunConsoleMain(int /*argc*/,
                                      char* /*argv*/[],
                                      void* sandbox_info,
                                      cef_version_info_t* /*version_info*/) {
  if (!sandbox_info) {
    std::fputs("CEF bootstrap did not supply sandbox information\n", stderr);
    return 64;
  }

  // Sandboxed subprocesses may have no standard handles. Their RunProbe path
  // dispatches CefExecuteProcess before touching input/output; the browser
  // path still rejects missing input. Preserve bytes on every available pipe.
  for (FILE* stream : {stdin, stdout}) {
    const int descriptor = _fileno(stream);
    if (descriptor >= 0 && _setmode(descriptor, _O_BINARY) == -1) {
      std::fputs("Cannot configure binary probe stdio\n", stderr);
      return 64;
    }
  }

  // The console ABI has no HINSTANCE argument. Use the client DLL's instance,
  // as the pinned console sample does, without changing its reference count.
  HMODULE instance = nullptr;
  if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                             GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                         reinterpret_cast<LPCWSTR>(&RunConsoleMain),
                         &instance)) {
    std::fputs("Cannot locate probe client module\n", stderr);
    return 64;
  }
  return RunProbe(CefMainArgs(instance), sandbox_info);
}
