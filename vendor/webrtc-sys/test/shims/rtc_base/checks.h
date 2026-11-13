// Stand-in for libwebrtc's rtc_base/checks.h, for the hardware-free VAAPI
// tests in this folder.
#ifndef TEST_SHIMS_RTC_BASE_CHECKS_H_
#define TEST_SHIMS_RTC_BASE_CHECKS_H_
#include <cassert>
#define RTC_DCHECK_LT(a, b) assert((a) < (b))
#endif
