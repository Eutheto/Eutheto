// Copyright (c) 2013 The Chromium Embedded Framework Authors.
// Portions copyright (c) 2010 The Chromium Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE.txt file.

#import <Cocoa/Cocoa.h>

#include <atomic>
#include <cstdint>
#include <cstdlib>
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>
#if defined(__arm64__)
#include <mach/arm/thread_status.h>
#elif defined(__x86_64__)
#include <mach/i386/thread_status.h>
#endif

#include <functional>
#include <utility>

#include "host.h"
#include "include/cef_application_mac.h"
#include "include/wrapper/cef_library_loader.h"

#if !defined(CEF_USE_SANDBOX)
#error "The macOS feasibility probe requires the CEF sandbox"
#endif

@interface CefProbeApplication : NSApplication <CefAppProtocol> {
 @private
  BOOL handlingSendEvent_;
  BOOL terminationRequested_;
  std::function<void()> closeHandler_;
}
- (void)setProbeCloseHandler:(std::function<void()>)handler;
@end

@implementation CefProbeApplication
- (BOOL)isHandlingSendEvent {
  return handlingSendEvent_;
}

- (void)setHandlingSendEvent:(BOOL)handlingSendEvent {
  handlingSendEvent_ = handlingSendEvent;
}

- (void)sendEvent:(NSEvent*)event {
  CefScopedSendingEvent sendingEventScoper;
  [super sendEvent:event];
}

- (void)terminate:(id)sender {
  // Cocoa's default exit() would bypass OnBeforeClose and CefShutdown.
  terminationRequested_ = YES;
  if (closeHandler_) {
    closeHandler_();
  }
}

- (void)setProbeCloseHandler:(std::function<void()>)handler {
  closeHandler_ = std::move(handler);
  if (terminationRequested_ && closeHandler_) {
    closeHandler_();
  }
}
@end

@interface CefProbeDelegate : NSObject <NSApplicationDelegate>
@end

@implementation CefProbeDelegate
- (BOOL)applicationSupportsSecureRestorableState:(NSApplication*)application {
  return YES;
}
@end

// The shared host installs this only while its CEF client is alive.
void SetProbeCloseHandler(std::function<void()> handler) {
  [static_cast<CefProbeApplication*>(NSApp)
      setProbeCloseHandler:std::move(handler)];
}

namespace {
static_assert(std::atomic<bool>::is_always_lock_free);
#if defined(__arm64__)
using ProbeRegisters = arm_thread_state64_t;
constexpr thread_state_flavor_t kProbeFlavor = ARM_THREAD_STATE64;
constexpr mach_msg_type_number_t kProbeRegisterCount = ARM_THREAD_STATE64_COUNT;
constexpr uintptr_t kFrameAlignment = 16;
#elif defined(__x86_64__)
using ProbeRegisters = x86_thread_state64_t;
constexpr thread_state_flavor_t kProbeFlavor = x86_THREAD_STATE64;
constexpr mach_msg_type_number_t kProbeRegisterCount = x86_THREAD_STATE64_COUNT;
constexpr uintptr_t kFrameAlignment = 8;
#else
#error "Shutdown diagnostics require native arm64 or x86_64"
#endif

struct ShutdownDiagnostic {
  thread_t main_thread;
  uintptr_t stack_low, stack_high;
  std::atomic<bool> finished{false};
  decltype(&write) write_fault = &write;
  decltype(&_exit) exit_fault = &_exit;
};

bool EqualDiagnosticName(const char* value, const char* literal) {
  if (!value) return false;
  for (size_t index = 0; index < 128; ++index) {
    if (value[index] != literal[index]) return false;
    if (literal[index] == '\0') return true;
  }
  return false;
}

// dladdr reports a NEAREST symbol, not a containing function or causal stack.
// Only exact system-module basenames and inspected C API names are admitted.
unsigned NearestDiagnostic(uintptr_t address) {
  Dl_info info{};
  if (!address || !dladdr(reinterpret_cast<const void*>(address), &info) ||
      !info.dli_fname || !info.dli_sname) return 7;
  const char* module = info.dli_fname;
  size_t length = 0;
  for (; length < 1024 && info.dli_fname[length]; ++length) {
    if (info.dli_fname[length] == '/') module = info.dli_fname + length + 1;
  }
  if (length == 1024) return 7;
  if (EqualDiagnosticName(module, "libsystem_kernel.dylib")) {
    if (EqualDiagnosticName(info.dli_sname, "mach_msg2_trap") ||
        EqualDiagnosticName(info.dli_sname, "mach_msg_trap") ||
        EqualDiagnosticName(info.dli_sname, "mach_msg_overwrite_trap") ||
        EqualDiagnosticName(info.dli_sname, "mach_msg") ||
        EqualDiagnosticName(info.dli_sname, "mach_msg_overwrite")) return 0;
    if (EqualDiagnosticName(info.dli_sname, "__psynch_cvwait")) return 2;
    if (EqualDiagnosticName(info.dli_sname, "semaphore_wait_trap") ||
        EqualDiagnosticName(info.dli_sname, "semaphore_timedwait_trap") ||
        EqualDiagnosticName(info.dli_sname, "semaphore_wait_signal_trap") ||
        EqualDiagnosticName(info.dli_sname, "semaphore_timedwait_signal_trap")) return 3;
    if (EqualDiagnosticName(info.dli_sname, "__ulock_wait") ||
        EqualDiagnosticName(info.dli_sname, "__ulock_wait2")) return 4;
  } else if (EqualDiagnosticName(module, "libsystem_pthread.dylib")) {
    if (EqualDiagnosticName(info.dli_sname, "pthread_join")) return 1;
    if (EqualDiagnosticName(info.dli_sname, "pthread_cond_wait") ||
        EqualDiagnosticName(info.dli_sname, "pthread_cond_timedwait") ||
        EqualDiagnosticName(info.dli_sname, "_pthread_cond_wait")) return 2;
  } else if (EqualDiagnosticName(module, "libdispatch.dylib")) {
    if (EqualDiagnosticName(info.dli_sname, "dispatch_semaphore_wait") ||
        EqualDiagnosticName(info.dli_sname, "_dispatch_semaphore_wait_slow") ||
        EqualDiagnosticName(info.dli_sname, "dispatch_group_wait") ||
        EqualDiagnosticName(info.dli_sname, "_dispatch_group_wait_slow")) return 5;
  } else if (EqualDiagnosticName(module, "AudioToolbox")) {
    if (EqualDiagnosticName(info.dli_sname, "AudioComponentInstanceDispose")) return 6;
  }
  return 7;
}

void* ObserveShutdown(void* opaque) {
  auto& diagnostic = *static_cast<ShutdownDiagnostic*>(opaque);
  ProbeRegisters registers{};
  uintptr_t addresses[64]{}, frame[2]{};
  mach_vm_size_t copied = 0;
  const task_t task = mach_task_self();

  // Warm Mach RPC stubs and this observer's MIG reply port while main can run.
  // libsyscall builds these clients with -novouchers; their VM copies are kernel
  // operations, not malloc or raw frame-pointer dereferences in this process.
  (void)thread_suspend(MACH_PORT_NULL);
  (void)thread_resume(MACH_PORT_NULL);
  const thread_t observer = mach_thread_self();
  mach_msg_type_number_t count = kProbeRegisterCount;
  const kern_return_t warmed_state = thread_get_state(
      observer, kProbeFlavor, reinterpret_cast<thread_state_t>(&registers), &count);
  (void)mach_port_deallocate(task, observer);
  const kern_return_t warmed_read = mach_vm_read_overwrite(
      task, reinterpret_cast<mach_vm_address_t>(addresses), sizeof(frame),
      reinterpret_cast<mach_vm_address_t>(frame), &copied);
  if (warmed_state != KERN_SUCCESS || warmed_read != KERN_SUCCESS ||
      copied != sizeof(frame)) {
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    return nullptr;
  }
  for (unsigned tick = 0; tick < 100; ++tick) {
    if (diagnostic.finished.load(std::memory_order_acquire)) return nullptr;
    const timespec delay{0, 10 * 1000 * 1000};
    (void)nanosleep(&delay, nullptr);
  }
  if (diagnostic.finished.load(std::memory_order_acquire)) return nullptr;
  if (thread_suspend(diagnostic.main_thread) != KERN_SUCCESS) {
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    return nullptr;
  }

  // Suspended section: lock-free flag, warmed Mach APIs and fixed stack buffers
  // only. No allocation, dyld/symbol lookup, pthread lookup, stdio or emission.
  size_t used = 0;
  bool incomplete = false;
  if (!diagnostic.finished.load(std::memory_order_acquire)) {
    count = kProbeRegisterCount;
    const kern_return_t state_result = thread_get_state(
        diagnostic.main_thread, kProbeFlavor,
        reinterpret_cast<thread_state_t>(&registers), &count);
    uintptr_t pc = 0, fp = 0, sp = 0;
    if (state_result == KERN_SUCCESS && count == kProbeRegisterCount) {
#if defined(__arm64__) && !__DARWIN_OPAQUE_ARM_THREAD_STATE64
      pc = arm_thread_state64_get_pc(registers);
      fp = arm_thread_state64_get_fp(registers);
      sp = arm_thread_state64_get_sp(registers);
#elif defined(__x86_64__)
      pc = registers.__rip;
      fp = registers.__rbp;
      sp = registers.__rsp;
#endif
      // Opaque/PAC thread states are unsupported rather than authenticated,
      // stripped or guessed. Signed saved return addresses likewise stay raw;
      // failed dladdr lookup after resume yields only the unknown category.
      if (pc && sp >= diagnostic.stack_low && sp < diagnostic.stack_high) {
        addresses[used++] = pc;
        while (fp && used < 64) {
          if (fp < sp || fp < diagnostic.stack_low ||
              fp > diagnostic.stack_high - sizeof(frame) ||
              fp % kFrameAlignment != 0) {
            incomplete = true;
            break;
          }
          copied = 0;
          if (mach_vm_read_overwrite(task, fp, sizeof(frame),
                  reinterpret_cast<mach_vm_address_t>(frame), &copied) != KERN_SUCCESS ||
              copied != sizeof(frame)) {
            incomplete = true;
            break;
          }
          if (!frame[1]) {
            incomplete = true;
            break;
          }
          addresses[used++] = frame[1];
          if (!frame[0]) break;
          if (frame[0] <= fp) {
            incomplete = true;
            break;
          }
          fp = frame[0];
        }
        if (used == 64) incomplete = true;
      }
    }
  }
  if (thread_resume(diagnostic.main_thread) != KERN_SUCCESS) {
    // Main may still hold libc/dyld locks. One best-effort POSIX write followed
    // by immediate POSIX _exit; no retry, stdio, cleanup or normal return.
    constexpr char fault[] = "diagnostic-self-stack-resume-failed\n";
    (void)diagnostic.write_fault(STDOUT_FILENO, fault, sizeof(fault) - 1);
    diagnostic.exit_fault(70);
    __builtin_unreachable();
  }
  if (diagnostic.finished.load(std::memory_order_acquire)) return nullptr;
  if (!used) {
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    return nullptr;
  }
  EmitProbeLifecycle("diagnostic-self-stack-captured\n");
  bool seen[8]{};
  seen[7] = incomplete;
  for (size_t index = 0; index < used; ++index) {
    seen[NearestDiagnostic(addresses[index])] = true;
  }
  constexpr const char* tokens[] = {
      "diagnostic-nearest-mach-message\n", "diagnostic-nearest-pthread-join\n",
      "diagnostic-nearest-condition-wait\n", "diagnostic-nearest-semaphore-wait\n",
      "diagnostic-nearest-ulock-wait\n", "diagnostic-nearest-dispatch-wait\n",
      "diagnostic-nearest-audio-dispose\n", "diagnostic-nearest-unknown\n"};
  for (size_t index = 0; index < 8; ++index) {
    if (seen[index]) EmitProbeLifecycle(tokens[index]);
  }
  return nullptr;
}
}  // namespace

void ShutdownProbe() {
  const char* mode = std::getenv("EUTHETO_PROBE_SHUTDOWN_DIAGNOSTIC");
  if (!mode || mode[0] != '1' || mode[1] != '\0') {
    CefShutdown();
    return;
  }
  if (!pthread_main_np()) {
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    CefShutdown();
    return;
  }
  const pthread_t main = pthread_self();
  const uintptr_t high = reinterpret_cast<uintptr_t>(pthread_get_stackaddr_np(main));
  const size_t size = pthread_get_stacksize_np(main);
  const thread_t port = mach_thread_self();
  if (!MACH_PORT_VALID(port) || size < 2 * sizeof(uintptr_t) || size > high) {
    if (MACH_PORT_VALID(port)) (void)mach_port_deallocate(mach_task_self(), port);
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    CefShutdown();
    return;
  }
  ShutdownDiagnostic diagnostic{port, high - size, high};
  pthread_t observer;
  if (pthread_create(&observer, nullptr, ObserveShutdown, &diagnostic) != 0) {
    (void)mach_port_deallocate(mach_task_self(), port);
    EmitProbeLifecycle("diagnostic-self-stack-unavailable\n");
    CefShutdown();
    return;
  }
  CefShutdown();
  diagnostic.finished.store(true, std::memory_order_release);
  // Keep the stack-owned state and main-thread Mach send right until the sole
  // joinable observer has finished. Process exit cannot leave a sampler alive.
  if (pthread_join(observer, nullptr) != 0) diagnostic.exit_fault(70);
  (void)mach_port_deallocate(mach_task_self(), port);
}

int main(int argc, char* argv[]) {
  int result;
  {
    // Keep the framework loaded until CEF shutdown and Cocoa pool drainage.
    CefScopedLibraryLoader libraryLoader;
    if (!libraryLoader.LoadInMain()) {
      return 1;
    }

    @autoreleasepool {
      CefProbeApplication* application = [CefProbeApplication sharedApplication];
      if (![application isKindOfClass:[CefProbeApplication class]]) {
        return 1;
      }
      [application setActivationPolicy:NSApplicationActivationPolicyProhibited];
      __attribute__((objc_precise_lifetime)) CefProbeDelegate* delegate =
          [[CefProbeDelegate alloc] init];
      application.delegate = delegate;

      result = RunProbe(CefMainArgs(argc, argv), nullptr);
      EmitProbeLifecycle("lifecycle-probe-returned\n");

      SetProbeCloseHandler({});
      application.delegate = nil;
    }
    EmitProbeLifecycle("lifecycle-pool-drained\n");
    EmitProbeLifecycle("lifecycle-unload-entered\n");
  }
  EmitProbeLifecycle("lifecycle-unload-returned\n");
  return result;
}
