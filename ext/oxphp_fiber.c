/* ext/oxphp_fiber.c — Fiber scheduler implementation.
 *
 * Stub file — implementations will be added in Task 7.
 * This file exists to verify the header compiles and to be
 * included in the PHP extension build (config.m4). */

#include "oxphp_fiber.h"

__thread oxphp_request_fiber *oxphp_current_fiber = NULL;
