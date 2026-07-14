<?php
// Runner-side test: an application-set X-Frame-Options must survive untouched,
// and the server must NOT add its Content-Security-Policy: frame-ancestors
// fallback (a server CSP would override the app's framing choice in modern
// browsers). The runner asserts the response headers — see suites/headers.txt.
header('X-Frame-Options: DENY');
echo "ok";
