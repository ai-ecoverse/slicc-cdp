({
  mouse(args, { up, deepest, owner, held, focusable }) {
    const tree = (node) => {
      const chain = [];
      for (let at = node; at; at = up(at)) if (at instanceof Element) chain.unshift(at);
      return chain;
    };
    const shared = (a, b) => {
      const chain = tree(a);
      for (let at = b; at; at = up(at)) if (chain.includes(at)) return at;
      return null;
    };
    const pointer = { pointerId: 1, pointerType: 'mouse', isPrimary: true };
    const base = { composed: true, view: window, ...held(args.modifiers) };
    const fire = (node, type, init, quiet) => {
      const kind = type.startsWith('pointer');
      const extra = kind ? pointer : {};
      const options = { ...base, bubbles: !quiet, cancelable: !quiet, ...extra, ...init };
      return node.dispatchEvent(new (kind ? PointerEvent : MouseEvent)(type, options));
    };
    const both = (node, suffix, init, quiet) => {
      fire(node, `pointer${suffix}`, { ...init, button: -1 }, quiet);
      fire(node, `mouse${suffix}`, { ...init, button: 0 }, quiet);
    };
    const cross = (from, to, init) => {
      const away = from?.isConnected ? tree(from) : [];
      const into = tree(to);
      if (away.length) both(from, 'out', init);
      for (const node of away.filter((at) => !into.includes(at)).reverse())
        both(node, 'leave', init, true);
      both(to, 'over', init);
      for (const node of into.filter((at) => !away.includes(at))) both(node, 'enter', init, true);
    };
    const clicks = [];
    let pressed = args.press ? deepest(...args.press) : null;
    const down = (target, init) => {
      fire(target, 'pointerdown', init);
      pressed = target;
      if (!fire(target, 'mousedown', { ...init, detail: 1 })) return;
      const next = target.closest(focusable);
      if (next && !next.disabled) next.focus({ preventScroll: true });
      else document.activeElement?.blur();
    };
    const release = (target, init) => {
      fire(target, 'pointerup', init);
      fire(target, 'mouseup', { ...init, detail: 1 });
      const common = pressed && shared(pressed, target);
      const type = init.button ? 'auxclick' : 'click';
      if (common) clicks.push(() => fire(common, type, { ...init, detail: 1 }));
      pressed = null;
    };
    const act = {
      move: (target, init) => both(target, 'move', init),
      down,
      up: release,
    };
    const hits = args.steps.map(([, x, y]) => deepest(x, y));
    const blocked = hits.findIndex((hit) => owner(hit));
    if (blocked >= 0) return `frame:${blocked}`;
    if (!hits.every(Boolean)) return 'missing';
    let last = args.from ? deepest(...args.from) : null;
    for (const [kind, x, y, buttons, button] of args.steps) {
      const target = deepest(x, y) ?? document.documentElement;
      const init = { clientX: x, clientY: y, buttons, button };
      if (target !== last) cross(last, target, init);
      last = target;
      act[kind](target, init);
    }
    const channel = new MessageChannel();
    channel.port1.onmessage = () => {
      channel.port1.close();
      for (const click of clicks) click();
    };
    channel.port2.postMessage(0);
    return 'done';
  },
  keys(args, target, { edit, editable, held }) {
    const activatable = 'a[href],area[href],button,input,select,summary,[role=button]';
    const erasers = { Backspace: 'delete', Delete: 'forwardDelete' };
    const enter = args.key === 'Enter';
    const typed = !!args.text && !enter && args.key !== 'Tab' && !(args.modifiers & 6);
    const button = !editable(target) && !!target.closest(activatable);
    const init = {
      key: args.key,
      code: args.code,
      bubbles: true,
      cancelable: true,
      composed: true,
      view: window,
      repeat: !!args.repeat,
      location: args.location ?? 0,
      ...held(args.modifiers),
    };
    const send = (type) => target.dispatchEvent(new KeyboardEvent(type, init));
    const fallback = () => {
      if (typed) return edit(target, 'insertText', args.text);
      if (!(target instanceof HTMLInputElement))
        return edit(target, target.isContentEditable ? 'insertParagraph' : 'insertLineBreak');
      target.form?.requestSubmit();
    };
    const down = () => {
      const erase = erasers[args.key];
      const go = send('keydown') && editable(target);
      if (go && erase) edit(target, erase);
      else if (go && args.key !== 'Escape' && send('keypress')) fallback();
    };
    if (args.phase === 'up') {
      if (args.key === ' ' && button) return 'unsupported';
      send('keyup');
      return 'pressed';
    }
    const plain =
      typed || enter || /^(Backspace|Delete|Escape|Alt|Control|Meta|Shift)$/.test(args.key);
    if (((enter || args.key === ' ') && button) || !plain) return 'unsupported';
    down();
    if (args.phase !== 'down') send('keyup');
    return 'pressed';
  },
});
