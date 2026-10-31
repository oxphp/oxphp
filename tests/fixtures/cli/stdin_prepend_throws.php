<?php
// auto_prepend_file for php_cli_options.php's stdin-prepend mode.
register_shutdown_function(function () {
    echo is_array(@fstat(STDIN)) ? 'stdin-open' : 'stdin-closed';
});
throw new Error('prepend');
