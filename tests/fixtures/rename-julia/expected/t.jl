function renamed(x)
    y = x + bump
    return y
end

function renamed(x, y)
    x * y
end

bump = 1

function main()
    a = renamed(2)
    for i in 1:3
        a += i
    end
    println(a)
end
