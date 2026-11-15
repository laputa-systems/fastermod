subroutine demo(x)
  integer :: x, renamed
  renamed = x + 1
  call helper(renamed)
end subroutine demo
