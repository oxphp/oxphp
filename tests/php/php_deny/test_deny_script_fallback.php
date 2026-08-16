<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_script_fallback_placeholder', 'php_deny');
// Placeholder — the runner uses a URL override to hit /uploads/shell.php,
// which oxphp rewrites to execute public/_security/denied.php. The actual
// assertions (OXPHP_DENIED_PATH, OXPHP_DENIED_PATTERN) live in denied.php
// and are merged by the runner via its JSON-body detection.
$t->assertTrue('placeholder — real assertions live in _security/denied.php', true);
$t->done();
