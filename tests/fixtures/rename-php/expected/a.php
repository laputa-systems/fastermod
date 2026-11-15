<?php
function helper($x) {
  $y = $x + 1;
  return $y;
}
$total = helper(1);
if ($total > 0) { $inner = $total; }
function useit() {
  $count = 9;
  return $count;
}
