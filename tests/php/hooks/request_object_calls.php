<?php

declare(strict_types=1);

// One call of every public method of OxPHP\Http\Request, keyed by the method's
// name, for the tests that check which request a Request object answers for.
// Closures rather than declared functions: the worker keeps a declared function
// for its whole life, so a second include would redeclare it.

use OxPHP\Http\Request;

return [
    'method'              => static fn(Request $r) => $r->method(),
    'path'                => static fn(Request $r) => $r->path(),
    'fullUri'             => static fn(Request $r) => $r->fullUri(),
    'scheme'              => static fn(Request $r) => $r->scheme(),
    'host'                => static fn(Request $r) => $r->host(),
    'port'                => static fn(Request $r) => $r->port(),
    'queryString'         => static fn(Request $r) => $r->queryString(),
    'isSecure'            => static fn(Request $r) => $r->isSecure(),
    'isMethod'            => static fn(Request $r) => $r->isMethod('POST'),
    'httpProtocol'        => static fn(Request $r) => $r->httpProtocol(),
    'httpProtocolVersion' => static fn(Request $r) => $r->httpProtocolVersion(),
    'header'              => static fn(Request $r) => $r->header('content-type'),
    'headers'             => static fn(Request $r) => $r->headers(),
    'hasHeader'           => static fn(Request $r) => $r->hasHeader('content-type'),
    'cookie'              => static fn(Request $r) => $r->cookie('ox_who'),
    'cookies'             => static fn(Request $r) => $r->cookies(),
    'body'                => static fn(Request $r) => $r->body(),
    'contentType'         => static fn(Request $r) => $r->contentType(),
    'ip'                  => static fn(Request $r) => $r->ip(),
    'startTime'           => static fn(Request $r) => $r->startTime(true),
    'query'               => static fn(Request $r) => $r->query('who'),
    'payload'             => static fn(Request $r) => $r->payload('who'),
    'attributes'          => static fn(Request $r) => $r->attributes(),
    'session'             => static fn(Request $r) => $r->session(),
    'file'                => static fn(Request $r) => $r->file('upload'),
    'files'               => static fn(Request $r) => $r->files(),
];
