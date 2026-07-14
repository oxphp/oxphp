<?php
// Runner-side test: with FRAME_OPTIONS unset (the default profile), the server
// must emit the SAMEORIGIN clickjacking pair — X-Frame-Options: SAMEORIGIN plus
// Content-Security-Policy: frame-ancestors 'self' — and X-Content-Type-Options:
// nosniff on every response. Does NOT use TestCase/done(): the assertions are on
// the response headers the server adds, checked by the runner (suites/headers.txt).
echo "ok";
