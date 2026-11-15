module math
contains
  integer function bump(x)
    integer, intent(in) :: x
    bump = x + 1
  end function bump
  subroutine use_it()
    integer :: y
    y = bump(1)
  end subroutine use_it
end module math
