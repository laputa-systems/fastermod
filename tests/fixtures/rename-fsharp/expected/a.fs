let bump x = x + 1
let renamed y = bump y + old_name
let main () =
    let local = 5
    let helper = 9
    printfn "%d" (helper local + helper)
    helper 2 |> ignore
