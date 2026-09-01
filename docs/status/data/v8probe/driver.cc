// Minimal PDFium+V8 JS driver using ONLY the public C API.
// Mirrors testing/pdfium_test/pdfium_test.cc's ExampleAppAlert / Init flow,
// but lets PDFium create its own v8::Isolate + v8::Platform (m_pIsolate =
// m_pPlatform = NULL), so no V8 headers / static lib are required.
//
// Build:
//   g++ -std=c++17 -O2 driver.cc -o driver \
//       -I <extract>/include -L <extract>/lib -lpdfium \
//       -Wl,-rpath,<extract>/lib
//
// Usage: ./driver file.pdf

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#include "fpdfview.h"
#include "fpdf_formfill.h"
#include "fpdf_doc.h"

// Convert a NUL-terminated UTF-16LE FPDF_WIDESTRING to a UTF-8-ish narrow
// string (ASCII-only messages in our tests, so a byte-drop is fine).
static std::string WideToNarrow(FPDF_WIDESTRING ws) {
  std::string out;
  if (!ws) return out;
  const unsigned char* p = reinterpret_cast<const unsigned char*>(ws);
  for (;;) {
    unsigned int cp = p[0] | (p[1] << 8);
    if (cp == 0) break;
    if (cp < 0x80) out.push_back(static_cast<char>(cp));
    else out.push_back('?');
    p += 2;
  }
  return out;
}

// Mirror of ExampleAppAlert: prints "<title>: <msg>" to stdout.
static int ExampleAppAlert(IPDF_JSPLATFORM*,
                           FPDF_WIDESTRING msg,
                           FPDF_WIDESTRING title,
                           int type,
                           int icon) {
  printf("%s", WideToNarrow(title).c_str());
  if (icon || type) printf("[icon=%d,type=%d]", icon, type);
  printf(": %s\n", WideToNarrow(msg).c_str());
  fflush(stdout);
  return 0;
}

static double now_ms() {
  using namespace std::chrono;
  return duration<double, std::milli>(
             steady_clock::now().time_since_epoch())
      .count();
}

int main(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s file.pdf\n", argv[0]);
    return 2;
  }
  const char* path = argv[1];

  // PDFium creates its own V8 isolate + platform (both NULL).
  FPDF_LIBRARY_CONFIG config;
  memset(&config, 0, sizeof(config));
  config.version = 3;               // version 3 exposes m_pPlatform
  config.m_pUserFontPaths = nullptr;
  config.m_pIsolate = nullptr;      // -> PDFium creates its own isolate
  config.m_v8EmbedderSlot = 0;
  config.m_pPlatform = nullptr;     // -> PDFium creates its own platform
  FPDF_InitLibraryWithConfig(&config);

  FPDF_DOCUMENT doc = FPDF_LoadDocument(path, nullptr);
  if (!doc) {
    fprintf(stderr, "FPDF_LoadDocument failed: err=%lu\n", FPDF_GetLastError());
    FPDF_DestroyLibrary();
    return 1;
  }

  IPDF_JSPLATFORM platform_callbacks;
  memset(&platform_callbacks, 0, sizeof(platform_callbacks));
  platform_callbacks.version = 3;
  platform_callbacks.app_alert = ExampleAppAlert;

  FPDF_FORMFILLINFO ffi;
  memset(&ffi, 0, sizeof(ffi));
  // The prebuilt binary is an XFA build (pdf_enable_xfa = true); its
  // CheckFormfillVersion() requires version 2. xfa_disabled=0 keeps XFA on,
  // but our test PDFs are plain AcroForm/JS so XFA is inert here.
  ffi.version = 2;
  ffi.xfa_disabled = 0;
  ffi.m_pJsPlatform = &platform_callbacks;

  FPDF_FORMHANDLE form = FPDFDOC_InitFormFillEnvironment(doc, &ffi);
  if (!form) {
    fprintf(stderr, "FPDFDOC_InitFormFillEnvironment failed\n");
    FPDF_CloseDocument(doc);
    FPDF_DestroyLibrary();
    return 1;
  }

  fprintf(stderr, "[driver] running document JS actions...\n");
  fflush(stderr);
  double t0 = now_ms();
  FORM_DoDocumentJSAction(form);
  FORM_DoDocumentOpenAction(form);
  double t1 = now_ms();
  fprintf(stderr, "[driver] doc JS actions done: %.3f ms\n", t1 - t0);

  // Load page 0 and fire its OPEN additional-action (page-level JS).
  double t2 = now_ms();
  FPDF_PAGE page = FPDF_LoadPage(doc, 0);
  if (page) {
    FORM_OnAfterLoadPage(page, form);
    FORM_DoPageAAction(page, form, FPDFPAGE_AACTION_OPEN);
  } else {
    fprintf(stderr, "[driver] FPDF_LoadPage(0) returned null\n");
  }
  double t3 = now_ms();
  fprintf(stderr, "[driver] page-0 JS actions done: %.3f ms\n", t3 - t2);

  if (page) {
    FORM_DoPageAAction(page, form, FPDFPAGE_AACTION_CLOSE);
    FORM_OnBeforeClosePage(page, form);
    FPDF_ClosePage(page);
  }
  FPDFDOC_ExitFormFillEnvironment(form);
  FPDF_CloseDocument(doc);
  FPDF_DestroyLibrary();
  fprintf(stderr, "[driver] clean exit\n");
  return 0;
}
