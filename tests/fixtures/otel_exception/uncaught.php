<?php
function processPayment(): void {
    throw new RuntimeException('uncaught path: gateway down');
}
processPayment();
