# CEF 14c5a089: tests/ceftests/CMakeLists.txt.in selects bootstrapc.exe
# for a console client DLL; SDK cef_variables/macros own assets and LPAC ACLs.
if(NOT USE_SANDBOX OR NOT "CEF_USE_BOOTSTRAP" IN_LIST CEF_COMPILER_DEFINES)
  message(FATAL_ERROR "The Windows probe requires USE_SANDBOX and CEF_USE_BOOTSTRAP")
endif()
if(NOT MSVC OR NOT PROJECT_ARCH STREQUAL "x86_64")
  message(FATAL_ERROR "The pinned Windows validation target requires MSVC x64")
endif()

# Honor the harness's build parallelism cap, including the SDK wrapper.
list(REMOVE_ITEM CEF_COMPILER_FLAGS /MP)

# Only the staged runtime subtree receives LPAC read/execute permission. Refuse
# an in-checkout build so this post-build ACL cannot affect repository files.
file(REAL_PATH "${CMAKE_CURRENT_LIST_DIR}/../.." _probe_checkout_dir)
file(REAL_PATH "${CMAKE_BINARY_DIR}" _probe_build_dir)
cmake_path(IS_PREFIX _probe_checkout_dir "${_probe_build_dir}" NORMALIZE _probe_in_checkout)
if(_probe_in_checkout)
  message(FATAL_ERROR "Build the probe outside the checkout in its owned work directory")
endif()

function(add_cef_probe_targets)
  if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_BUILD_TYPE STREQUAL "Release")
    message(FATAL_ERROR "The pinned minimal SDK requires a Release probe build")
  endif()

  ADD_LOGICAL_TARGET(libcef_lib "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")
  set(CEF_TARGET_OUT_DIR "${CMAKE_BINARY_DIR}/$<CONFIG>")
  add_library(cef-probe SHARED
    "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/entry_windows.cc"
    ${PROBE_SHARED_SOURCES})
  SET_LIBRARY_TARGET_PROPERTIES(cef-probe)
  set_target_properties(cef-probe PROPERTIES
    PREFIX ""
    OUTPUT_NAME "cef-probe"
    RUNTIME_OUTPUT_DIRECTORY "${CEF_TARGET_OUT_DIR}"
    LIBRARY_OUTPUT_DIRECTORY "${CEF_TARGET_OUT_DIR}"
    VS_DEBUGGER_COMMAND "${CEF_TARGET_OUT_DIR}/cef-probe.exe")
  target_compile_definitions(cef-probe PRIVATE
    CEF_PROBE_RUNTIME_DIR="$<TARGET_FILE_DIR:cef-probe>")
  add_dependencies(cef-probe libcef_dll_wrapper)
  target_link_libraries(cef-probe PRIVATE
    libcef_lib libcef_dll_wrapper ${CEF_STANDARD_LIBS})

  COPY_SINGLE_FILE(cef-probe
    "${CEF_BINARY_DIR}/bootstrapc.exe" "${CEF_TARGET_OUT_DIR}/cef-probe.exe")
  COPY_FILES(cef-probe "${CEF_BINARY_FILES}" "${CEF_BINARY_DIR}" "${CEF_TARGET_OUT_DIR}")
  COPY_FILES(cef-probe "${CEF_RESOURCE_FILES}" "${CEF_RESOURCE_DIR}" "${CEF_TARGET_OUT_DIR}")
  COPY_FILES(cef-probe "LICENSE.txt;CREDITS.html" "${CEF_ROOT}" "${CEF_TARGET_OUT_DIR}")

  # SDK SID S-1-15-2-2 is All Restricted Application Packages. Do not grant it
  # access to the checkout, private cwd/input/profile, or build root.
  SET_LPAC_ACLS(cef-probe)
endfunction()
