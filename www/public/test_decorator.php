<?php
// Verify classes exist
var_dump(interface_exists('OxPHP\Decorator\AttributeInterface'));
var_dump(class_exists('OxPHP\Decorator\Context'));
var_dump(class_exists('OxPHP\Decorator\RejectedException'));
var_dump(function_exists('oxphp_register_decorator'));

// Verify interface methods
$ref = new ReflectionClass('OxPHP\Decorator\AttributeInterface');
var_dump($ref->hasMethod('before'));
var_dump($ref->hasMethod('after'));
echo "DECORATOR_SYSTEM_OK\n";
