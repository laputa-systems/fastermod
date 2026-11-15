<?php
function helper($x) { return $x; }
$a = renamed(1, 2);
$b = renamed($x);
$c = other(1);
// helper(9) in comment
echo $a;
