let bump x = x + 1
let renamed y = bump y + old_name
let () =
  let local = 5 in
  let helper = 9 in
  Printf.printf "%d\n" (helper local + helper)
let z = renamed 2
