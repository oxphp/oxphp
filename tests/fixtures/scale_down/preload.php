<?php
// No-op OPcache preload. The dev image's baked oxphp.ini sets
// opcache.preload=/var/www/html/preload.php; mounting this fixture dir as the
// docroot must satisfy that path or php_module_startup() fails at boot.
