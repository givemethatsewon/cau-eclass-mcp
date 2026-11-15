(expectedOrigin, expectedPath, action) => {
    if (location.origin !== expectedOrigin || location.pathname !== expectedPath) return false;

    const visible = element => element && element.getClientRects().length > 0 &&
        getComputedStyle(element).visibility === 'visible';

    if (action === 'open') {
        const save = document.querySelector('input.btnSAVEAS[title="Save"]');
        if (!visible(save) || save.disabled) return false;
        save.click();
        return true;
    }

    const dialog = [...document.querySelectorAll('[role="dialog"], dialog')].find(element =>
        visible(element) && element.querySelector('select.oz_ui_layout'));
    if (!dialog) return false;
    const format = dialog.querySelector('select.oz_ui_layout');
    const pdf = [...format.options].find(option => option.label.trim() === 'Adobe PDF File(*.pdf)');
    if (!pdf || pdf.disabled) return false;

    if (action === 'select') {
        format.value = pdf.value;
        format.dispatchEvent(new Event('input', {bubbles: true}));
        format.dispatchEvent(new Event('change', {bubbles: true}));
        return true;
    }

    if (format.selectedOptions[0] !== pdf) return false;
    const ok = [...dialog.querySelectorAll('button, input[type="button"], input[type="submit"], [role="button"]')]
        .find(element => (element.value || element.textContent).trim() === 'OK' && visible(element));
    if (!ok || ok.disabled || ok.getAttribute('aria-disabled') === 'true') return false;
    ok.click();
    return true;
}
