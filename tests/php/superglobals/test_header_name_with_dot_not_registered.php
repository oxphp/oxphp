<?php
// Sent: `X.Authenticated.User: admin` and no `X-Authenticated-User`.
// PHP turns the `.` into `_` when it registers the key, so the header would
// read as HTTP_X_AUTHENTICATED_USER — the identity an auth proxy sets — even
// with nothing to collide with. The object API still returns it under the
// name it was sent with.
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_name_with_dot_not_registered', 'superglobals');
$t->assertKeyMissing('X.Authenticated.User does not create HTTP_X_AUTHENTICATED_USER', $_SERVER, 'HTTP_X_AUTHENTICATED_USER');
$t->assertSame('header("X.Authenticated.User") returns the value', oxphp_http_request()->header('X.Authenticated.User'), 'admin');
$t->done();
