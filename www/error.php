<?php

trait Loggable
{
    public function log(): string
    {
        return static::class . ': logged';
    }
}

trait JsonExportable
{
    public function toJson(): string
    {
        return json_encode($this);
    }
}

class User
{
    use Loggable;
    use JsonExportable;

    public function __construct(
        public string $name,
        public string $email,
    ) {}
}

class Admin extends User
{
    use Loggable;
    use JsonExportable {
        JsonExportable::toJson as private parentToJson;
    }

    public function toJson(): string
    {
        return strtoupper($this->parentToJson());
    }
}

$user = new User('Alice', 'alice@example.com');
echo $user->log() . "\n";
echo $user->toJson() . "\n";

$admin = new Admin('Bob', 'bob@admin.com');
echo $admin->log() . "\n";
echo $admin->toJson() . "\n";

echo $undefined_var;

// Uncaught exception — generates stack trace
$user->nonExistentMethod();
