package main

func helper(x int) int { return x }

func main() {
	count := helper(1)
	_ = count
	// helper in comment
	s := "helper in string"
	_ = s
}
