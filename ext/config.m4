PHP_ARG_ENABLE([oxphp_sapi],
  [whether to enable OxPHP SAPI extension],
  [AS_HELP_STRING([--enable-oxphp-sapi],
    [Enable OxPHP SAPI extension])],
  [yes])

if test "$PHP_OXPHP_SAPI" != "no"; then
  dnl The extension reports the server's version: OXPHP_VERSION, which the
  dnl Dockerfiles also patch into Cargo.toml for the binary, else ../Cargo.toml.
  AC_ARG_VAR([OXPHP_VERSION],
    [OxPHP version the extension reports (default: the version in ../Cargo.toml)])
  AC_MSG_CHECKING([for the OxPHP version])
  AS_VAR_IF([OXPHP_VERSION], [],
    [OXPHP_VERSION=$(sed -n 's/^version = "\(.*\)".*/\1/p' "$abs_srcdir/../Cargo.toml" 2>/dev/null | head -n 1)])
  AS_VAR_IF([OXPHP_VERSION], [],
    [AC_MSG_ERROR([cannot read the version from $abs_srcdir/../Cargo.toml; pass OXPHP_VERSION=<version> to configure])])
  AC_MSG_RESULT([$OXPHP_VERSION])
  AC_DEFINE_UNQUOTED([PHP_OXPHP_SAPI_VERSION], ["$OXPHP_VERSION"],
    [OxPHP server version the extension reports])

  PHP_ADD_INCLUDE([bridge])
  PHP_ADD_LIBRARY_WITH_PATH([oxphp_bridge], [/usr/local/lib], [OXPHP_SAPI_SHARED_LIBADD])
  PHP_SUBST([OXPHP_SAPI_SHARED_LIBADD])
  PHP_NEW_EXTENSION([oxphp_sapi], [oxphp_sapi.c oxphp_fiber.c], [$ext_shared])
fi
