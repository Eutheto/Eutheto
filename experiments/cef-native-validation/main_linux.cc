#include "host.h"

int main(int argc, char** argv) {
  return RunProbe(CefMainArgs(argc, argv), nullptr);
}
