// Stand-in for libwebrtc's rtc_base/logging.h, for the hardware-free VAAPI
// tests in this folder: RTC_LOG(sev) << ... goes to stderr only when
// TINHAT_VAAPI_TEST_LOG is set.
#ifndef TEST_SHIMS_RTC_BASE_LOGGING_H_
#define TEST_SHIMS_RTC_BASE_LOGGING_H_
#include <cstdlib>
#include <iostream>
#include <sstream>
namespace test_shim {
struct LogLine {
  std::ostringstream s;
  ~LogLine() {
    if (std::getenv("TINHAT_VAAPI_TEST_LOG")) std::cerr << s.str() << "\n";
  }
};
}  // namespace test_shim
#define RTC_LOG(sev) test_shim::LogLine().s
#endif
