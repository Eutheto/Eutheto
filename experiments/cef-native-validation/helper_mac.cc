// Copyright (c) 2013 The Chromium Embedded Framework Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that can
// be found in the LICENSE.txt file.

#include "include/cef_app.h"
#include "include/cef_sandbox_mac.h"
#include "include/wrapper/cef_library_loader.h"

#if !defined(CEF_USE_SANDBOX)
#error "The macOS feasibility helper requires the CEF sandbox"
#endif

int main(int argc, char* argv[]) {
  // Sandbox initialization must precede framework loading. Destruction order
  // keeps the sandbox context alive until after the framework is unloaded.
  CefScopedSandboxContext sandboxContext;
  if (!sandboxContext.Initialize(argc, argv)) {
    return 1;
  }

  CefScopedLibraryLoader libraryLoader;
  if (!libraryLoader.LoadInHelper()) {
    return 1;
  }

  // Helpers never call RunProbe or consume its browser-only stdin protocol.
  return CefExecuteProcess(CefMainArgs(argc, argv), nullptr, nullptr);
}
