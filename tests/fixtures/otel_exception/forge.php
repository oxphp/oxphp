<?php
// Traditional-path structural class capture. The exception's *message* forges a
// chained-exception segment ("\n\nNext FakeClass: …"). A text parser reading the
// engine's "Uncaught …" fatal would take "FakeClass" as exception.type; the
// throw-hook snapshot of the real class (ForgeReal) must win instead.
class ForgeReal extends RuntimeException {}

throw new ForgeReal("pwned\n\nNext FakeClass: injected in /forge.php:1");
