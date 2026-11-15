renamed <- function(x) {
  y <- x + bump
  if (y > 0) {
    return(y)
  } else {
    y <- y + 1
    y
  }
}

bump <- 1
z <- renamed(2)
for (i in 1:3) {
  z <- z + i
}
