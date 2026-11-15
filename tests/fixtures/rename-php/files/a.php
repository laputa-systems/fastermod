<?php
function helper($x) {
  $y = $x + 1;
  return $y;
}
$count = helper(1);
if ($count > 0) { $inner = $count; }
function useit() {
  $count = 9;
  return $count;
}
