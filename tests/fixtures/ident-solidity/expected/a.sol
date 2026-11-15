contract C {
  uint renamed;
  function helper() public returns (uint) { return renamed; }
}
