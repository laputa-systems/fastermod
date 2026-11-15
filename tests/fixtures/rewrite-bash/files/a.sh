#!/bin/sh
helper() {
  echo "$@"
}
helper 1 2
count=$(helper x)
# helper(9) in comment
