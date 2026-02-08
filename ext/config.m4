PHP_ARG_ENABLE([oxphp_sapi],
  [whether to enable OxPHP SAPI extension],
  [AS_HELP_STRING([--enable-oxphp-sapi],
    [Enable OxPHP SAPI extension])],
  [yes])

if test "$PHP_OXPHP_SAPI" != "no"; then
  PHP_ADD_INCLUDE([bridge])
  PHP_ADD_LIBRARY_WITH_PATH([oxphp_bridge], [/usr/local/lib], [OXPHP_SAPI_SHARED_LIBADD])
  PHP_SUBST([OXPHP_SAPI_SHARED_LIBADD])
  PHP_NEW_EXTENSION([oxphp_sapi], [oxphp_sapi.c], [$ext_shared])
fi
