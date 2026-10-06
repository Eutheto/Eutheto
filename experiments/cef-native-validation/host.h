// Synthetic feasibility experiment, not a product export interface.
#pragma once
#include "include/cef_app.h"

// Owns CEF initialization, message loop and shutdown. Windows/Linux dispatch
// subprocesses before reading stdin. macOS helpers execute CEF directly.
int RunProbe(const CefMainArgs& args, void* sandbox_info);

#if defined(OS_MAC)
#include <functional>
// Owned by the Cocoa application; an early termination request remains pending.
void SetProbeCloseHandler(std::function<void()> handler);
#endif
