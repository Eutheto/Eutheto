// Copyright (c) 2013 The Chromium Embedded Framework Authors.
// Portions copyright (c) 2010 The Chromium Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE.txt file.

#import <Cocoa/Cocoa.h>

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

int main(int argc, char* argv[]) {
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

    const int result = RunProbe(CefMainArgs(argc, argv), nullptr);

    SetProbeCloseHandler({});
    application.delegate = nil;
    return result;
  }
}
