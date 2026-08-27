! /// conda-script
! channels = ["https://prefix.dev/conda-forge"]
! entrypoint = "gfortran -o ${CACHE}/main ${SCRIPT} -llapack -lblas && ${CACHE}/main"
!
! [dependencies]
! gfortran = "*"
! liblapack = "*"
! /// end-conda-script
program solve
  implicit none
  real(8) :: a(2, 2), b(2)
  integer :: ipiv(2), info

  a = reshape([2.0d0, 1.0d0, 1.0d0, 3.0d0], [2, 2])
  b = [5.0d0, 10.0d0]
  call dgesv(2, 1, a, 2, ipiv, b, 2, info)
  write (*, '(a, i0)') 'info = ', info
  write (*, '(a, 2f8.3)') 'x =', b
end program solve
