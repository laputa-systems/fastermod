package main

func helper(x int) int { return x }

func main() {
	count := helper(1)
	helper := 5
	_ = helper
	_ = count
}
