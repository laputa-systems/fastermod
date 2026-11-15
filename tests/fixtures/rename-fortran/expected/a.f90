module math
contains
  integer function renamed(x)
    integer, intent(in) :: x
    renamed = x + 1
  end function renamed
  subroutine use_it()
    integer :: y
    y = renamed(1)
  end subroutine use_it
end module math
