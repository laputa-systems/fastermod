count = 0

def bump():
    global count
    count += 1
    return count

from mod import helper

def useit():
    return helper(count)
