<?php

declare(strict_types=1);

// exit() out of a function whose local holds a PDO object with a transaction
// open. exit() unwinds the way an exception does, so the rollback that object's
// destructor reads is reached with it pending, and what this request prints
// after the exit() says whether it got past that read.
(static function (): void {
    $pdo = new PDO(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
    );
    $pdo->beginTransaction();
    $pdo->exec('DO 1');
    echo 'exiting;';
    exit();
})();
echo 'after-exit;';
