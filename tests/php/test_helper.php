<?php

declare(strict_types=1);

class TestCase
{
    private string $test;
    private string $group;
    /** @var list<array{name: string, pass: bool, expected?: string, actual?: string}> */
    private array $assertions = [];
    /** @var array<string, mixed> */
    private array $meta = [];

    public function __construct(string $test, string $group)
    {
        $this->test = $test;
        $this->group = $group;
        set_error_handler(function (int $errno, string $errstr, string $errfile, int $errline) {
            throw new \ErrorException($errstr, 0, $errno, $errfile, $errline);
        });
        set_exception_handler(function (\Throwable $e) {
            $this->outputError($e->getMessage() . ' in ' . $e->getFile() . ':' . $e->getLine());
        });
        register_shutdown_function(function () {
            $error = error_get_last();
            if ($error && in_array($error['type'], [E_ERROR, E_PARSE, E_CORE_ERROR, E_COMPILE_ERROR])) {
                $this->outputError($error['message'] . ' in ' . $error['file'] . ':' . $error['line']);
            }
        });
    }

    public function assertEqual(string $name, mixed $actual, mixed $expected): void
    {
        $pass = $actual == $expected;
        $this->addAssertion($name, $pass, $expected, $actual);
    }

    public function assertSame(string $name, mixed $actual, mixed $expected): void
    {
        $pass = $actual === $expected;
        $this->addAssertion($name, $pass, $expected, $actual);
    }

    public function assertNotEqual(string $name, mixed $actual, mixed $unexpected): void
    {
        $pass = $actual != $unexpected;
        $this->addAssertion($name, $pass, "not " . $this->stringify($unexpected), $actual);
    }

    public function assertTrue(string $name, mixed $value): void
    {
        $this->addAssertion($name, $value === true, true, $value);
    }

    public function assertFalse(string $name, mixed $value): void
    {
        $this->addAssertion($name, $value === false, false, $value);
    }

    public function assertNull(string $name, mixed $value): void
    {
        $this->addAssertion($name, $value === null, null, $value);
    }

    public function assertNotNull(string $name, mixed $value): void
    {
        $pass = $value !== null;
        $this->addAssertion($name, $pass, "not null", $value);
    }

    public function assertContains(string $name, string $haystack, string $needle): void
    {
        $pass = str_contains($haystack, $needle);
        $this->addAssertion($name, $pass, "contains '$needle'", $haystack);
    }

    public function assertNotContains(string $name, string $haystack, string $needle): void
    {
        $pass = !str_contains($haystack, $needle);
        $this->addAssertion($name, $pass, "not contains '$needle'", $haystack);
    }

    public function assertKeyExists(string $name, array $array, string|int $key): void
    {
        $pass = array_key_exists($key, $array);
        $this->addAssertion($name, $pass, "key '$key' exists", array_keys($array));
    }

    public function assertKeyMissing(string $name, array $array, string|int $key): void
    {
        $pass = !array_key_exists($key, $array);
        $this->addAssertion($name, $pass, "key '$key' missing", array_keys($array));
    }

    public function assertMatch(string $name, string $value, string $pattern): void
    {
        $pass = (bool)preg_match($pattern, $value);
        $this->addAssertion($name, $pass, "matches $pattern", $value);
    }

    public function assertType(string $name, mixed $value, string $expectedType): void
    {
        $actualType = gettype($value);
        $pass = $actualType === $expectedType;
        $this->addAssertion($name, $pass, $expectedType, $actualType);
    }

    public function assertGreaterThan(string $name, int|float $actual, int|float $min): void
    {
        $pass = $actual > $min;
        $this->addAssertion($name, $pass, "> $min", $actual);
    }

    public function assertLessThan(string $name, int|float $actual, int|float $max): void
    {
        $pass = $actual < $max;
        $this->addAssertion($name, $pass, "< $max", $actual);
    }

    public function assertEmpty(string $name, mixed $value): void
    {
        $pass = empty($value);
        $this->addAssertion($name, $pass, "empty", $value);
    }

    public function assertNotEmpty(string $name, mixed $value): void
    {
        $pass = !empty($value);
        $this->addAssertion($name, $pass, "not empty", $value);
    }

    public function assertCount(string $name, array|\Countable $value, int $expected): void
    {
        $actual = count($value);
        $pass = $actual === $expected;
        $this->addAssertion($name, $pass, $expected, $actual);
    }

    public function assertInstanceOf(string $name, mixed $value, string $class): void
    {
        $pass = $value instanceof $class;
        $actual = is_object($value) ? get_class($value) : gettype($value);
        $this->addAssertion($name, $pass, $class, $actual);
    }

    public function assertThrows(string $name, callable $fn, string $exceptionClass): void
    {
        try {
            $fn();
            $this->addAssertion($name, false, "throws $exceptionClass", "no exception");
        } catch (\Throwable $e) {
            $pass = $e instanceof $exceptionClass;
            $actual = get_class($e) . ': ' . $e->getMessage();
            $this->addAssertion($name, $pass, "throws $exceptionClass", $actual);
        }
    }

    public function meta(string $key, mixed $value): void
    {
        $this->meta[$key] = $value;
    }

    public function done(): never
    {
        $pass = true;
        foreach ($this->assertions as $a) {
            if (!$a['pass']) {
                $pass = false;
                break;
            }
        }

        $this->output([
            'test'       => $this->test,
            'group'      => $this->group,
            'pass'       => $pass,
            'assertions' => $this->assertions,
            'error'      => null,
            'meta'       => empty($this->meta) ? new \stdClass() : $this->meta,
        ]);
    }

    private function addAssertion(string $name, bool $pass, mixed $expected, mixed $actual): void
    {
        $entry = ['name' => $name, 'pass' => $pass];
        if (!$pass) {
            $entry['expected'] = $this->stringify($expected);
            $entry['actual'] = $this->stringify($actual);
        }
        $this->assertions[] = $entry;
    }

    private function stringify(mixed $value): string
    {
        if (is_null($value)) return 'null';
        if (is_bool($value)) return $value ? 'true' : 'false';
        if (is_string($value)) return $value;
        if (is_int($value) || is_float($value)) return (string)$value;
        return json_encode($value, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
    }

    private function output(array $data): never
    {
        header('Content-Type: application/json');
        echo json_encode($data, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES | JSON_PRETTY_PRINT);
        exit(0);
    }

    private function outputError(string $message): never
    {
        $this->output([
            'test'       => $this->test,
            'group'      => $this->group,
            'pass'       => false,
            'assertions' => $this->assertions,
            'error'      => $message,
            'meta'       => empty($this->meta) ? new \stdClass() : $this->meta,
        ]);
    }
}
