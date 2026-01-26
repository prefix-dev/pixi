// Grab-able quotes scrolling functionality
document.addEventListener("DOMContentLoaded", () => {
    const scroll = document.querySelector('.quote-scroll-wrapper');
    if (!scroll) return;

    let isDown = false;
    let startX, scrollLeft;

    scroll.addEventListener('mousedown', (e) => {
        isDown = true;
        scroll.classList.add('dragging');
        startX = e.pageX - scroll.offsetLeft;
        scrollLeft = scroll.scrollLeft;
    });

    scroll.addEventListener('mouseleave', () => {
        isDown = false;
        scroll.classList.remove('dragging');
    });

    scroll.addEventListener('mouseup', () => {
        isDown = false;
        scroll.classList.remove('dragging');
    });

    scroll.addEventListener('mousemove', (e) => {
        if (!isDown) return;
        e.preventDefault();
        const x = e.pageX - scroll.offsetLeft;
        const walk = (x - startX) * 1;
        scroll.scrollLeft = scrollLeft - walk;
    });
});
