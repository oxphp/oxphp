<?php

declare(strict_types=1);

require_once __DIR__ . '/../tests/test_helper.php';

// `private/` names the directory, and `/private` without the slash is not a
// directory request: the rule lets it through and the directory index
// answers. The documentation lists this gap; a change here should be a
// decision, not an accident.
$t = new TestCase('test_dir_rule_misses_bare_uri', 'oxphpdeny');
$t->assertSame('REQUEST_URI', $_SERVER['REQUEST_URI'] ?? '', '/private');
$t->done();
