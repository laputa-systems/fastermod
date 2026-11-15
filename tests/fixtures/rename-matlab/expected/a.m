function y = renamed(x)
    local = x + 1;
    y = bump(local) + old_name;
end
function y = bump(x)
    y = x + 1;
end
z = renamed(1);
w = renamed.name;
