total = 0

def bump():
    global total
    total += 1
    return total

from mod import helper

def useit():
    return helper(total)
