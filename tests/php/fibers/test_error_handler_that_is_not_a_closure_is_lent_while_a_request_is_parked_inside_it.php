<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A request parked inside a call of its error handler lends the worker that
// handler, whatever callable it was set as. For one that is not a Closure, the
// call is told by the name of the function its frame runs, so each form below
// names it differently from that function's own name, or not at all.

$run = bin2hex(random_bytes(4));

// No set_error_handler() here: what is measured is the worker's handler.
$probe = static function (string $tag): array {
    ob_start();
    try {
        trigger_error("probe-$tag", E_USER_WARNING);
        $thrown = 'none';
    } catch (\Throwable $e) {
        $thrown = get_class($e) . ':' . $e->getMessage();
    }

    return ['thrown' => $thrown, 'shown' => (string) ob_get_clean()];
};

$t = new TestCase('error_handler_that_is_not_a_closure_is_lent_while_a_request_is_parked_inside_it', 'fibers');

foreach (['method', 'static', 'function', 'invoke', 'alias'] as $form) {
    $tag = "$form-$run";
    $in = static fn (): bool => OxphpErrorModeProbe::$inWindow["form-$tag"] ?? false;
    $parked = OxphpErrorModeProbe::send("mode=forms&form=$form&run=$tag", 'fixture_error_handler_park.php');
    for ($i = 0; $i < 100 && !$in(); $i++) {
        usleep(20_000);
    }
    $before = $in();
    $seen = $probe($tag);
    $inHandler = $before && $in();
    $parkedResp = OxphpErrorModeProbe::read($parked);

    $t->assertTrue("$form: premise: the other request was parked inside its handler while this one raised", $inHandler);
    $t->assertSame(
        "$form: a request served meanwhile: the handler hears its warning",
        $seen['thrown'],
        "ErrorException:form-handler:$form:probe-$tag"
    );
    $t->assertContains("$form: the other request ran to its end", $parkedResp, "DONE\n");
}

$t->done();
