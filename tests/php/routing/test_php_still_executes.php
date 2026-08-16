<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('php_still_executes', 'routing');
// In SPA mode PHP files should still execute normally.
// The fact this file runs and produces a response proves PHP execution
// works alongside the SPA fallback behaviour.
$t->assertTrue('PHP file executed in SPA mode', true);
$t->assertKeyExists('SCRIPT_NAME key exists', $_SERVER, 'SCRIPT_NAME');
$t->assertContains('SCRIPT_NAME contains filename', $_SERVER['SCRIPT_NAME'], 'test_php_still_executes.php');
$t->done();
