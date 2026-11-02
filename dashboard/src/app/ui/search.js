// Ctrl+K (⌘K on a Mac) puts the cursor in the topbar's search box, as the
// hint on it promises. Nothing else: the box is a plain GET form.
addEventListener("keydown", (event) => {
  if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "k") {
    return;
  }
  const box = document.getElementById("topbar-search");
  if (!box) {
    return;
  }
  event.preventDefault();
  box.focus();
  box.select();
});
