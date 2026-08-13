<?php

declare(strict_types=1);

// A file whose only job is to declare a class at the top level, so that
// requiring it a second time raises "Cannot declare class …" —  E_COMPILE_ERROR,
// a real zend_bailout that no user error handler can intercept.
//
// It exists because a `class` statement cannot be written inside a method body
// at all: the compiler rejects a nested class declaration while the enclosing
// file is compiled, which makes the file fatal on the way in rather than at the
// point that was meant to fatal. Requiring this one is how a method reaches the
// same E_COMPILE_ERROR at runtime.
//
// require, never require_once: re-executing the declaration is the whole point.
// And the file must exist — requiring a missing one emits E_WARNING first, which
// a set_error_handler left installed by an earlier request on this worker turns
// into a thrown ErrorException before any fatal is reached.

class BreakerRedeclareTarget
{
}
