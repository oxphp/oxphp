<?php

// Without strict_types on purpose: a Stringable object is accepted for PDO's DSN
// only from a file that does not declare them, which is the only way PDO ever
// sees one.

function oxphp_test_pdo_from_stringable(string $dsn, string $user, string $pass, array $options): PDO
{
    $name = new class ($dsn) {
        public function __construct(private string $dsn)
        {
        }

        public function __toString(): string
        {
            return $this->dsn;
        }
    };

    return new PDO($name, $user, $pass, $options);
}
