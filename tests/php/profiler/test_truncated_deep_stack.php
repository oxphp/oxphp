<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// A profiled request whose call chain is deeper than the bridge's
// 32-entry open-stack mirror (40 nested frames) but whose span count stays
// far below PROFILER_MAX_SPANS. Overflowing that mirror costs the heap
// hook its attribution path and leaves the frames past the 32nd unclosed,
// so finalize force-closes them and marks them leaked — present in the
// tree, but with an end stamp the observer never took. What it does not do
// is lose a span the way the span cap does, so the run must come out with
// truncated=false. profiler/test_truncated_index
// asserts both halves: a build that reported the mirror overflow as
// truncation would flag this run as truncated although nothing was
// dropped from it.

$t = new TestCase('truncated_deep_stack', 'profiler');

// Distinct functions rather than recursion: the observer's end callback
// matches the frame it pops by zend_function*, so a chain of one
// recursive function past the mirror's depth pops frames it did not push.
function _ds_01(): int { return _ds_02() + 1; }
function _ds_02(): int { return _ds_03() + 1; }
function _ds_03(): int { return _ds_04() + 1; }
function _ds_04(): int { return _ds_05() + 1; }
function _ds_05(): int { return _ds_06() + 1; }
function _ds_06(): int { return _ds_07() + 1; }
function _ds_07(): int { return _ds_08() + 1; }
function _ds_08(): int { return _ds_09() + 1; }
function _ds_09(): int { return _ds_10() + 1; }
function _ds_10(): int { return _ds_11() + 1; }
function _ds_11(): int { return _ds_12() + 1; }
function _ds_12(): int { return _ds_13() + 1; }
function _ds_13(): int { return _ds_14() + 1; }
function _ds_14(): int { return _ds_15() + 1; }
function _ds_15(): int { return _ds_16() + 1; }
function _ds_16(): int { return _ds_17() + 1; }
function _ds_17(): int { return _ds_18() + 1; }
function _ds_18(): int { return _ds_19() + 1; }
function _ds_19(): int { return _ds_20() + 1; }
function _ds_20(): int { return _ds_21() + 1; }
function _ds_21(): int { return _ds_22() + 1; }
function _ds_22(): int { return _ds_23() + 1; }
function _ds_23(): int { return _ds_24() + 1; }
function _ds_24(): int { return _ds_25() + 1; }
function _ds_25(): int { return _ds_26() + 1; }
function _ds_26(): int { return _ds_27() + 1; }
function _ds_27(): int { return _ds_28() + 1; }
function _ds_28(): int { return _ds_29() + 1; }
function _ds_29(): int { return _ds_30() + 1; }
function _ds_30(): int { return _ds_31() + 1; }
function _ds_31(): int { return _ds_32() + 1; }
function _ds_32(): int { return _ds_33() + 1; }
function _ds_33(): int { return _ds_34() + 1; }
function _ds_34(): int { return _ds_35() + 1; }
function _ds_35(): int { return _ds_36() + 1; }
function _ds_36(): int { return _ds_37() + 1; }
function _ds_37(): int { return _ds_38() + 1; }
function _ds_38(): int { return _ds_39() + 1; }
function _ds_39(): int { return _ds_40() + 1; }
function _ds_40(): int { return 1; }

OxPHP\Profile\start();
$depth = _ds_01();
$t->assertSame('chain ran to the bottom', $depth, 40);

$t->done();
