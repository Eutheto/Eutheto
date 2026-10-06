# Copyright (c) 2014 The Chromium Embedded Framework Authors. All rights
# reserved. Use of this source code is governed by a BSD-style license that
# can be found in the LICENSE.txt file.

# Included immediately after find_package(CEF), BEFORE creating the wrapper.
if(NOT USE_SANDBOX)
  message(FATAL_ERROR "The macOS feasibility probe requires USE_SANDBOX=ON")
endif()

# The pinned framework requires macOS 13, not the SDK CMake default of 12.
# Updating only CMAKE_OSX_DEPLOYMENT_TARGET leaves the SDK's explicit flag stale.
list(REMOVE_ITEM CEF_COMPILER_FLAGS "-mmacosx-version-min=12.0")
list(APPEND CEF_COMPILER_FLAGS "-mmacosx-version-min=13.0")
set(CEF_TARGET_SDK "13.0")
set(CMAKE_OSX_DEPLOYMENT_TARGET "13.0" CACHE STRING "Probe minimum macOS" FORCE)
set(CMAKE_OSX_DEPLOYMENT_TARGET "13.0")

# Never invoke Xcode automatic signing or any codesign/notarization command.
set(CMAKE_XCODE_ATTRIBUTE_CODE_SIGN_IDENTITY "")
set(CMAKE_XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED "NO")
set(CMAKE_XCODE_ATTRIBUTE_CODE_SIGNING_REQUIRED "NO")

function(add_cef_probe_targets)
  set(_source_dir "${CMAKE_CURRENT_FUNCTION_LIST_DIR}")
  set(_output_dir "${CMAKE_BINARY_DIR}/$<CONFIG>")
  set(_app "${_output_dir}/cef-probe.app")

  set(PROBE_EXECUTABLE_NAME "cef-probe")
  set(PROBE_BUNDLE_IDENTIFIER "org.eutheto.experiment.cef-probe")
  set(PROBE_PRINCIPAL_CLASS "CefProbeApplication")
  configure_file("${_source_dir}/Info.plist.in"
                 "${CMAKE_CURRENT_BINARY_DIR}/cef-probe-Info.plist" @ONLY)

  # Use the SDK's CXX treatment of .mm files so its CXX flags also apply to
  # Objective-C++. ARC is local to our entry, not to the SDK wrapper sources.
  set_source_files_properties("${_source_dir}/entry_mac.mm" PROPERTIES
    COMPILE_OPTIONS "-fobjc-arc")
  add_executable(cef-probe MACOSX_BUNDLE
    "${_source_dir}/entry_mac.mm" ${PROBE_SHARED_SOURCES})
  SET_EXECUTABLE_TARGET_PROPERTIES(cef-probe)
  target_link_libraries(cef-probe PRIVATE libcef_dll_wrapper ${CEF_STANDARD_LIBS})
  set_target_properties(cef-probe PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${_output_dir}"
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_BINARY_DIR}/cef-probe-Info.plist"
    XCODE_ATTRIBUTE_CLANG_ENABLE_OBJC_ARC "YES")

  # Includes all framework libraries/locales/paks and SDK-required symlinks.
  # Do not link the framework directly: helpers must sandbox before loading it.
  COPY_MAC_FRAMEWORK(cef-probe "${CEF_BINARY_DIR}" "${_app}")

  # The pinned SDK requires base, Alerts, GPU, Plugin and Renderer helpers.
  foreach(_suffix_list IN LISTS CEF_HELPER_APP_SUFFIXES)
    string(REPLACE ":" ";" _suffix_list "${_suffix_list}")
    list(GET _suffix_list 0 _name_suffix)
    list(GET _suffix_list 1 _target_suffix)
    list(GET _suffix_list 2 _plist_suffix)
    set(_helper_target "cef-probe-helper${_target_suffix}")
    set(_helper_name "cef-probe Helper${_name_suffix}")

    set(PROBE_EXECUTABLE_NAME "${_helper_name}")
    set(PROBE_BUNDLE_IDENTIFIER
        "org.eutheto.experiment.cef-probe.helper${_plist_suffix}")
    set(PROBE_PRINCIPAL_CLASS "NSApplication")
    set(_helper_plist "${CMAKE_CURRENT_BINARY_DIR}/${_helper_target}-Info.plist")
    configure_file("${_source_dir}/Info.plist.in" "${_helper_plist}" @ONLY)

    add_executable(${_helper_target} MACOSX_BUNDLE "${_source_dir}/helper_mac.cc")
    SET_EXECUTABLE_TARGET_PROPERTIES(${_helper_target})
    target_link_libraries(${_helper_target} PRIVATE
      libcef_dll_wrapper ${CEF_STANDARD_LIBS})
    set_target_properties(${_helper_target} PROPERTIES
      OUTPUT_NAME "${_helper_name}"
      RUNTIME_OUTPUT_DIRECTORY "${_output_dir}"
      MACOSX_BUNDLE_INFO_PLIST "${_helper_plist}")
    add_dependencies(cef-probe ${_helper_target})
    add_custom_command(TARGET cef-probe POST_BUILD
      COMMAND "${CMAKE_COMMAND}" -E copy_directory
              "$<TARGET_BUNDLE_DIR:${_helper_target}>"
              "${_app}/Contents/Frameworks/${_helper_name}.app"
      VERBATIM)
  endforeach()
endfunction()
