#ifndef PHP_OXPHP_SAPI_H
#define PHP_OXPHP_SAPI_H

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"

#define PHP_OXPHP_SAPI_VERSION "0.1.0"
#define PHP_OXPHP_SAPI_EXTNAME "oxphp_sapi"

extern zend_module_entry oxphp_sapi_module_entry;
#define phpext_oxphp_sapi_ptr &oxphp_sapi_module_entry

PHP_MINIT_FUNCTION(oxphp_sapi);

PHP_FUNCTION(oxphp_request_id);
PHP_FUNCTION(oxphp_worker_id);
PHP_FUNCTION(oxphp_server_info);
PHP_FUNCTION(oxphp_request_heartbeat);
PHP_FUNCTION(oxphp_finish_request);
PHP_FUNCTION(oxphp_is_streaming);
PHP_FUNCTION(oxphp_stream_flush);
PHP_FUNCTION(oxphp_worker);

ZEND_FUNCTION(oxphp_plugin_dispatch);

PHP_FUNCTION(oxphp_async);
PHP_FUNCTION(oxphp_async_await);
PHP_FUNCTION(oxphp_async_await_all);
PHP_FUNCTION(oxphp_async_await_any);

#endif /* PHP_OXPHP_SAPI_H */
