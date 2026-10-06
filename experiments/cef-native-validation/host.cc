// Disposable synthetic CEF feasibility probe; not a product export host.
#include "host.h"
#include "include/cef_client.h"
#include "include/cef_request_context.h"
#include "include/cef_devtools_message_observer.h"
#include "include/cef_parser.h"
#include "include/base/cef_callback.h"
#include "include/wrapper/cef_byte_read_handler.h"
#include "include/wrapper/cef_closure_task.h"
#include "include/wrapper/cef_stream_resource_handler.h"
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <string>

namespace {
constexpr char kUrl[] = "https://eutheto-report.invalid/report";
constexpr size_t kLimit = 16 * 1024 * 1024;
void Event(const char* event) {
  const size_t size = std::char_traits<char>::length(event);
  if (std::fwrite(event, 1, size, stdout) != size || std::fflush(stdout) != 0)
    std::_Exit(70);
}
std::string Utf8(const std::filesystem::path& path) {
  const auto bytes = path.u8string();
  return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size());
}
class Html final : public CefBaseRefCounted {
 public:
  explicit Html(std::string value) : bytes(std::move(value)) {}
  const std::string bytes;
 private:
  IMPLEMENT_REFCOUNTING(Html);
};

class Client final : public CefClient, public CefLifeSpanHandler,
                     public CefLoadHandler, public CefRequestHandler,
                     public CefRequestContextHandler, public CefResourceRequestHandler,
                     public CefRenderHandler, public CefPrintHandler,
                     public CefPdfPrintCallback, public CefDownloadHandler,
                     public CefDisplayHandler, public CefDevToolsMessageObserver {
 public:
  Client(CefRefPtr<Html> html, std::string output) : html_(html), output_(std::move(output)) {}
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefRequestHandler> GetRequestHandler() override { return this; }
  CefRefPtr<CefRenderHandler> GetRenderHandler() override { return this; }
  CefRefPtr<CefPrintHandler> GetPrintHandler() override { return this; }
  CefRefPtr<CefDownloadHandler> GetDownloadHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }
  bool succeeded() const { return printed_ && closed_ && !failed_; }
  void Fail() {
    if (failed_.exchange(true)) return;
    Event("failed\n");
    CefPostTask(TID_UI, base::BindOnce(&Client::Close, CefRefPtr<Client>(this)));
  }
  void Close() {
    if (browser_) browser_->GetHost()->CloseBrowser(true);
    else CefQuitMessageLoop();
  }
  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override {
    if (browser_) { browser->GetHost()->CloseBrowser(true); Fail(); return; }
    browser_ = browser;
    browser_id_ = browser->GetIdentifier();
    Event("browser\n");
  }
  void OnBeforeClose(CefRefPtr<CefBrowser> browser) override {
    if (!browser_ || browser->GetIdentifier() != browser_id_) return;
    inspection_ = nullptr;
    browser_ = nullptr;
    closed_ = true;
    Event("closed\n");
    CefQuitMessageLoop();
  }
  bool OnBeforePopup(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>, int,
      const CefString&, const CefString&, CefLifeSpanHandler::WindowOpenDisposition, bool,
      const CefPopupFeatures&, CefWindowInfo&, CefRefPtr<CefClient>&,
      CefBrowserSettings&, CefRefPtr<CefDictionaryValue>&, bool*) override {
    Event("deny-popup\n"); Fail(); return true;
  }
  bool OnBeforeBrowse(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame,
      CefRefPtr<CefRequest> request, bool, bool redirect) override {
    if (navigated_ || redirect || !Allowed(browser, frame, request)) {
      Event("deny-navigation\n"); Fail(); return true;
    }
    navigated_ = true;
    return false;
  }
  bool OnOpenURLFromTab(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>, const CefString&,
      CefRequestHandler::WindowOpenDisposition, bool) override {
    Event("deny-tab\n"); Fail(); return true;
  }
  CefRefPtr<CefResourceRequestHandler> GetResourceRequestHandler(
      CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>, CefRefPtr<CefRequest>, bool,
      bool, const CefString&, bool& disable_default_handling) override {
    disable_default_handling = true;
    return this;
  }
  ReturnValue OnBeforeResourceLoad(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame,
      CefRefPtr<CefRequest> request, CefRefPtr<CefCallback>) override {
    if (Allowed(browser, frame, request)) return RV_CONTINUE;
    Event(request->GetResourceType() == RT_FAVICON ? "deny-favicon\n" : "deny-resource\n");
    Fail(); return RV_CANCEL;
  }
  CefRefPtr<CefResourceHandler> GetResourceHandler(CefRefPtr<CefBrowser> browser,
      CefRefPtr<CefFrame> frame, CefRefPtr<CefRequest> request) override {
    if (!Allowed(browser, frame, request) || served_.exchange(true)) {
      Event("deny-handler\n"); Fail(); return nullptr;
    }
    auto bytes = new CefByteReadHandler(
        reinterpret_cast<const unsigned char*>(html_->bytes.data()), html_->bytes.size(), html_);
    CefResponse::HeaderMap headers;
    headers.emplace("Content-Type", "text/html; charset=utf-8");
    return new CefStreamResourceHandler(200, "OK", "text/html", std::move(headers),
                                        CefStreamReader::CreateForHandler(bytes));
  }
  void OnProtocolExecution(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>,
      CefRefPtr<CefRequest>, bool& allow_os_execution) override {
    allow_os_execution = false;
    Event("deny-protocol\n"); Fail();
  }
  bool CanDownload(CefRefPtr<CefBrowser>, const CefString&, const CefString&) override {
    Event("deny-download\n"); Fail(); return false;
  }
  bool OnConsoleMessage(CefRefPtr<CefBrowser>, cef_log_severity_t, const CefString&,
      const CefString&, int) override { return true; }
  void GetViewRect(CefRefPtr<CefBrowser>, CefRect& rect) override { rect = CefRect(0, 0, 1280, 720); }
  void OnPaint(CefRefPtr<CefBrowser>, PaintElementType, const RectList&, const void*, int, int) override {
    // PDF output does not consume offscreen raster frames.
  }
  void OnPrintStart(CefRefPtr<CefBrowser>) override { if (!printing_) Fail(); }
  void OnPrintSettings(CefRefPtr<CefBrowser>, CefRefPtr<CefPrintSettings>, bool) override {
    if (!printing_) Fail();
  }
  bool OnPrintDialog(CefRefPtr<CefBrowser>, bool, CefRefPtr<CefPrintDialogCallback>) override {
    Event("deny-print-dialog\n"); Fail(); return false;
  }
  bool OnPrintJob(CefRefPtr<CefBrowser>, const CefString&, const CefString&,
      CefRefPtr<CefPrintJobCallback>) override {
    Event("deny-print-job\n"); Fail(); return false;
  }
  void OnPrintReset(CefRefPtr<CefBrowser>) override {
    // No physical-printer state: this host only admits PrintToPDF.
  }
  CefSize GetPdfPaperSize(CefRefPtr<CefBrowser>, int dpi) override {
    if (dpi <= 0 || dpi > 10000) { Fail(); return CefSize(); }
    return CefSize(dpi * 827 / 100, dpi * 1169 / 100);
  }
  void OnLoadEnd(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame, int status) override {
    if (!frame->IsMain()) return;
    if (waiting_ || !browser_ || browser->GetIdentifier() != browser_id_ ||
        frame->GetURL() != kUrl || status != 200 || failed_) { Fail(); return; }
    waiting_ = true;
    Event("loaded\n");
    inspection_ = browser->GetHost()->AddDevToolsMessageObserver(this);
    Inspect(1, "Page.getFrameTree", nullptr);
  }
  void OnLoadError(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>, ErrorCode,
      const CefString&, const CefString&) override { Fail(); }
  void OnRenderProcessTerminated(CefRefPtr<CefBrowser>, TerminationStatus, int,
      const CefString&) override { Event("renderer-exit\n"); Fail(); }
  void Inspect(int id, const char* method, CefRefPtr<CefDictionaryValue> params) {
    expected_inspection_ = id;
    if (!inspection_ || browser_->GetHost()->ExecuteDevToolsMethod(id, method, params) != id) Fail();
  }
  void OnDevToolsMethodResult(CefRefPtr<CefBrowser> browser, int id, bool success,
      const void* bytes, size_t size) override {
    if (failed_ || closed_) return;
    if (!waiting_ || printing_ || !browser_ || browser->GetIdentifier() != browser_id_ ||
        browser->GetMainFrame()->GetURL() != kUrl || id != expected_inspection_ ||
        !success || !bytes || size == 0 || size > 4096) { Fail(); return; }
    auto parsed = CefParseJSON(bytes, size, JSON_PARSER_RFC);
    if (!parsed || parsed->GetType() != VTYPE_DICTIONARY) { Fail(); return; }
    auto result = parsed->GetDictionary();
    if (id == 1) {
      auto tree = result->GetDictionary("frameTree");
      auto frame = tree ? tree->GetDictionary("frame") : nullptr;
      if (!frame || frame->GetType("id") != VTYPE_STRING || frame->GetString("url") != kUrl ||
          tree->HasKey("childFrames")) { Fail(); return; }
      auto params = CefDictionaryValue::Create();
      params->SetString("frameId", frame->GetString("id"));
      params->SetString("worldName", "eutheto-owned-inspection");
      params->SetBool("grantUniveralAccess", false);
      params->SetString("contentSecurityPolicy", "default-src 'none'; connect-src 'none'");
      Inspect(2, "Page.createIsolatedWorld", params);
      return;
    }
    if (id == 2) {
      if (result->GetType("executionContextId") != VTYPE_INT || result->GetInt("executionContextId") <= 0) {
        Fail(); return;
      }
      auto params = CefDictionaryValue::Create();
      params->SetInt("contextId", result->GetInt("executionContextId"));
      params->SetBool("awaitPromise", true);
      params->SetBool("returnByValue", true);
      params->SetString("expression",
          "(async () => { const root = document.getElementById('recipient-report'); "
          "if (!root || root.getAttribute('data-recipient-ready') !== 'true' || "
          "root.hasAttribute('data-recipient-error')) return false; "
          "const search = root.querySelector('input[type=search]'); "
          "if (!search) return false; search.value = 'eutheto-no-matching-person'; "
          "search.dispatchEvent(new Event('input', {bubbles:true})); "
          "const rows = root.querySelectorAll('[data-row]'); "
          "if (rows.length !== 64 || [...rows].some(row => !row.hidden)) return false; "
          "await document.fonts.ready; "
          "return location.href === 'https://eutheto-report.invalid/report' && "
          "document.readyState === 'complete' && document.fonts.status === 'loaded' && "
          "root.isConnected && root.getAttribute('data-recipient-ready') === 'true' && "
          "!root.hasAttribute('data-recipient-error'); })()");
      Inspect(3, "Runtime.evaluate", params);
      return;
    }
    auto value = result->GetDictionary("result");
    if (id != 3 || result->HasKey("exceptionDetails") || !value ||
        value->GetString("type") != "boolean" || value->GetType("value") != VTYPE_BOOL ||
        !value->GetBool("value")) { Fail(); return; }
    inspection_ = nullptr;
    expected_inspection_ = 0;
    Event("ready\n");
    if (std::getenv("EUTHETO_PROBE_HOLD_READY")) return;
    printing_ = true;
    CefPdfPrintSettings settings;
    settings.print_background = true;
    settings.prefer_css_page_size = true;
    settings.generate_tagged_pdf = true;
    browser_->GetHost()->PrintToPDF(output_, settings, this);
    if (!failed_ && !printed_) Event("printing\n");
  }
  void OnPdfPrintFinished(const CefString& path, bool ok) override {
    if (failed_ || !printing_ || printed_ || path != output_ || !ok) { Fail(); return; }
    printed_ = true;
    Event("printed\n");
    Close();
  }
 private:
  bool Allowed(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame,
      CefRefPtr<CefRequest> request) const {
    return !failed_ && browser && frame && frame->IsMain() &&
        browser->GetIdentifier() == browser_id_ && request->GetURL() == kUrl &&
        request->GetMethod() == "GET";
  }
  const CefRefPtr<Html> html_;
  const std::string output_;
  CefRefPtr<CefBrowser> browser_;
  CefRefPtr<CefRegistration> inspection_;
  int expected_inspection_ = 0;
  std::atomic<int> browser_id_{0};
  std::atomic<bool> served_{false}, failed_{false};
  bool navigated_ = false, waiting_ = false, printing_ = false, printed_ = false, closed_ = false;
  IMPLEMENT_REFCOUNTING(Client);
};


class App final : public CefApp, public CefBrowserProcessHandler {
 public:
  CefRefPtr<Client> client;
  CefRefPtr<CefBrowserProcessHandler> GetBrowserProcessHandler() override { return this; }
  void OnBeforeCommandLineProcessing(const CefString&, CefRefPtr<CefCommandLine> line) override {
#if defined(OS_LINUX)
    line->AppendSwitchWithValue("ozone-platform", "headless");
#endif
    if (std::getenv("EUTHETO_PROBE_POPUP_TEST")) line->AppendSwitch("disable-popup-blocking");
    line->AppendSwitch("disable-gpu");
    line->AppendSwitch("disable-background-networking");
    line->AppendSwitch("disable-component-update");
    line->AppendSwitch("disable-sync");
    line->AppendSwitch("no-first-run");
  }
  void OnContextInitialized() override {
    Event("context\n");
    CefWindowInfo window;
    window.SetAsWindowless(0);
    CefBrowserSettings settings;
    settings.windowless_frame_rate = 1;
    CefRequestContextSettings context_settings;
    auto context = CefRequestContext::CreateContext(context_settings, client);
    if (!CefBrowserHost::CreateBrowser(window, client, kUrl, settings, nullptr, context)) client->Fail();
  }
 private:
  IMPLEMENT_REFCOUNTING(App);
};
}  // namespace

#if defined(OS_MAC)
void EmitProbeLifecycle(const char* event) {
  Event(event);
}
#endif

int RunProbe(const CefMainArgs& args, void* sandbox_info) {
  CefRefPtr<App> app = new App;
#if !defined(OS_MAC)
  const int child = CefExecuteProcess(args, app, sandbox_info);
  if (child >= 0) return child;
#endif
  Event("startup\n");
  std::string html;
  char buffer[8192];
  while (std::cin.read(buffer, sizeof(buffer)) || std::cin.gcount()) {
    const auto count = static_cast<size_t>(std::cin.gcount());
    if (count > kLimit - html.size()) return 64;
    html.append(buffer, count);
  }
  if (!std::cin.eof() || html.empty()) return 64;
  // The Windows bootstrap changes cwd before entering the client DLL.
  // Restore only the browser's explicit owner-selected private directory.
#if defined(OS_WIN)
  const auto* job = _wgetenv(L"EUTHETO_PROBE_JOB_DIR");
#else
  const auto* job = std::getenv("EUTHETO_PROBE_JOB_DIR");
#endif
  if (!job || !*job) return 64;
  const std::filesystem::path directory(job);
  std::error_code error;
  if (!directory.is_absolute() || !std::filesystem::is_directory(directory, error) || error)
    return 64;
  std::filesystem::current_path(directory, error);
  if (error) return 64;
  app->client = new Client(new Html(std::move(html)), Utf8(directory / "output.pdf"));
  CefSettings settings;
  settings.no_sandbox = false;
  settings.command_line_args_disabled = true;
  settings.windowless_rendering_enabled = true;
  // Private synthetic diagnostics only; not a production logging policy.
  settings.log_severity = LOGSEVERITY_ERROR;
  CefString(&settings.log_file) = Utf8(directory / "cef.log");
  CefString(&settings.root_cache_path) = Utf8(directory / "profile");
#if !defined(OS_MAC)
  CefString(&settings.resources_dir_path) = CEF_PROBE_RUNTIME_DIR;
  CefString(&settings.locales_dir_path) = CEF_PROBE_RUNTIME_DIR "/locales";
#endif
  if (!CefInitialize(args, settings, app, sandbox_info)) return 71;
#if defined(OS_MAC)
  SetProbeCloseHandler([client = app->client] { client->Fail(); });
#endif
  CefRunMessageLoop();
#if defined(OS_MAC)
  EmitProbeLifecycle("lifecycle-loop-returned\n");
#endif
  CefRefPtr<Client> completed = app->client;
  app->client = nullptr;
#if defined(OS_MAC)
  SetProbeCloseHandler({});
  EmitProbeLifecycle("lifecycle-shutdown-entered\n");
#endif
  CefShutdown();
#if defined(OS_MAC)
  EmitProbeLifecycle("lifecycle-shutdown-returned\n");
#endif
  return completed->succeeded() ? 0 : 72;
}
