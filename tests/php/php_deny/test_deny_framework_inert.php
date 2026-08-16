<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_framework_inert_placeholder', 'php_deny');
// Placeholder — in Framework mode (INDEX_FILE=index.php), PHP_DENY_DIRS
// is inert. The runner uses a URL override to hit /uploads/shell.php,
// which oxphp rewrites to index.php (the front controller). The real
// assertions live in fixtures/php_deny/public/index.php and are merged
// by the runner via its JSON-body detection.
$t->assertTrue('placeholder — real assertions live in public/index.php', true);
$t->done();
