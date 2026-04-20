<?php
/**
 * Regression: before the fix, plugin-registered Exception subclasses
 * (TypeException, CapacityException, ...) received the Map class's
 * zend_object_handlers slot, which carries offset = sizeof(wrapper-prefix).
 * Plain Exception objects (allocated via zend_throw_exception without the
 * oxphp_custom_object prefix) then read that offset during GC/destroy and
 * computed a wild outer-struct pointer, SIGSEGV-ing on destroy.
 *
 * The failing pattern requires:
 *   (a) a live Shared\* with prior ops (so the heap has the right layout),
 *   (b) at least two back-to-back Shared\* constructor-throw catches, where
 *       `$e` is rebound each time and the old exception's destructor runs.
 */
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map(3);
$m->set('k1', 1);
$m->set('k2', 2);
$m->set('k3', 3);
try { $m->set('k4', 4); } catch (\Exception $e) {}
$m->remove('k1');
$m->set('k4', 4);  // refill to cap after remove — needed to reproduce
try { new OxPHP\Shared\Map(0); } catch (\Exception $e) {}
try { new OxPHP\Shared\Map(-5); } catch (\Exception $e) {}

// If we reach here without the worker dying the fix is in place.
if ($m->count() !== 3) { echo "FAIL: map state damaged\n"; exit; }
if ($m->get('k4') !== 4) { echo "FAIL: refill lost\n"; exit; }
echo "OK\n";
