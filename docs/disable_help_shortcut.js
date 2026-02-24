(() => {
  // Disable mdBook's "?" help shortcut so "?" can be typed freely in editors.
  window.addEventListener(
    "keydown",
    (event) => {
      const isQuestionShortcut =
        event.key === "?" || (event.code === "Slash" && event.shiftKey);
      const hasExtraModifiers = event.ctrlKey || event.metaKey || event.altKey;

      if (!isQuestionShortcut || hasExtraModifiers) {
        return;
      }

      event.stopImmediatePropagation();
    },
    true
  );
})();
