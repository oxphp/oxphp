<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// payload() takes a body for a form exactly when PHP would pick a form handler
// for it on POST: the type name up to the first ';', ',' or space, compared
// whole and ignoring case. The runner sends the body a=1 under various
// Content-Type values; ?expect=form or ?expect=none says which way it goes.
$t = new TestCase('payload_form_type', 'http_object');

$form = ($_GET['expect'] ?? '') === 'form';
$req = oxphp_http_request();

$t->assertSame('payload()', $req->payload(), $form ? ['a' => '1'] : null);
$t->assertSame('payload("a")', $req->payload('a', 'dflt'), $form ? '1' : 'dflt');

if ($req->method() === 'POST') {
    // On POST PHP itself decides, and payload() has to agree with it.
    $t->assertSame('$_POST agrees', $_POST, $form ? ['a' => '1'] : []);
} else {
    $t->assertSame('$_POST stays empty', $_POST, []);
}
$t->assertSame('body() is the raw body', $req->body(), 'a=1');

$t->done();
