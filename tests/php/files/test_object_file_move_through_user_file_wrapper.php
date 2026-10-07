<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The application has replaced the file:// wrapper with its own, as test tools
// that rewrite code on load do: a class that hands every call back to the
// built-in wrapper. The destination is relative and the rename cannot write
// into its directory, so move_uploaded_file() copies, opening the destination
// through that class. type() is first called from a shutdown function, after
// PHP has changed the working directory back, and must still read the file
// the copy wrote.
// Sent via curl as -F "doc=@small.txt" (contents "hello world").
$t = new TestCase('object_file_move_through_user_file_wrapper', 'files');

final class PassThroughFileWrapper
{
    /** @var resource|null */
    public $context;
    /** @var resource */
    private $handle;

    public static function install(): void
    {
        stream_wrapper_unregister('file');
        stream_wrapper_register('file', self::class);
    }

    private static function builtin(callable $fn): mixed
    {
        stream_wrapper_restore('file');
        try {
            return $fn();
        } finally {
            self::install();
        }
    }

    public function stream_open(string $path, string $mode, int $options, ?string &$openedPath): bool
    {
        $handle = self::builtin(static fn () => @fopen($path, $mode));
        if ($handle === false) {
            return false;
        }
        $this->handle = $handle;
        return true;
    }

    public function stream_read(int $count): string|false
    {
        return fread($this->handle, $count);
    }

    public function stream_write(string $data): int|false
    {
        return fwrite($this->handle, $data);
    }

    public function stream_eof(): bool
    {
        return feof($this->handle);
    }

    public function stream_seek(int $offset, int $whence): bool
    {
        return fseek($this->handle, $offset, $whence) === 0;
    }

    public function stream_tell(): int|false
    {
        return ftell($this->handle);
    }

    public function stream_flush(): bool
    {
        return fflush($this->handle);
    }

    public function stream_stat(): array|false
    {
        return fstat($this->handle);
    }

    public function stream_set_option(int $option, int $arg1, ?int $arg2): bool
    {
        return false;
    }

    /** @return resource */
    public function stream_cast(int $castAs)
    {
        return $this->handle;
    }

    public function stream_close(): void
    {
        fclose($this->handle);
    }

    public function url_stat(string $path, int $flags): array|false
    {
        return self::builtin(static fn () => ($flags & STREAM_URL_STAT_LINK) ? @lstat($path) : @stat($path));
    }
}

$req = oxphp_http_request();
$dir = '/tmp/oxphp_uf_userfile_' . uniqid('', true);
mkdir($dir);
file_put_contents($dir . '/a.txt', 'GIF89a');
chmod($dir, 0555);
$t->assertFalse('the rename cannot write into the directory', is_writable($dir));
$t->assertTrue('the copy can write the file already there', is_writable($dir . '/a.txt'));

$typeOf = static function ($f): string {
    try {
        return $f->type();
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};

chdir($dir);
$f = $req->file('doc');
PassThroughFileWrapper::install();
$moved = $f->moveTo('a.txt');
$contents = file_get_contents($dir . '/a.txt');
stream_wrapper_restore('file');
$t->assertSame('moveTo() by a relative path through the replaced wrapper', $moved, true);
$t->assertSame('the copy wrote the upload over the file', $contents, 'hello world');

register_shutdown_function(static function () use ($t, $f, $dir, $typeOf): void {
    $t->assertTrue('the working directory changed before shutdown', getcwd() !== $dir);
    PassThroughFileWrapper::install();
    $type = $typeOf($f);
    stream_wrapper_restore('file');
    $t->assertSame('type() from shutdown reads the file the copy wrote', $type, 'text/plain');

    chmod($dir, 0755);
    unlink($dir . '/a.txt');
    rmdir($dir);

    $t->done();
});
