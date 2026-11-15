<?php
function helper($x) { return $x; }
$a = helper(1, 2);
$b = helper($x);
$c = other(1);
// helper(9) in comment
echo $a;
