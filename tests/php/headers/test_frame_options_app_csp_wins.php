<?php
// Runner-side test: an application CSP that carries its own frame-ancestors
// directive owns the framing policy, so the server must NOT add its
// X-Frame-Options fallback (a stricter server XFO would over-block in legacy
// browsers that ignore CSP). The runner asserts the response headers — see
// suites/headers.txt.
header("Content-Security-Policy: frame-ancestors 'self' https://partner.example.com");
echo "ok";
