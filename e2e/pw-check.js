const { chromium } = require('playwright');
(async()=>{
  const browser = await chromium.launch({headless:true});
  const page = await browser.newPage();
  await page.goto('http://192.168.0.69/',{waitUntil:'domcontentloaded',timeout:30000});
  console.log('pathname', await page.evaluate(()=>location.pathname));
  console.log('cookies', JSON.stringify(await page.context().cookies('http://192.168.0.69/')));
  console.log('localStorageKeys', await page.evaluate(()=>Object.keys(localStorage)));
  console.log('nano token', await page.evaluate(()=>localStorage.getItem('nano-kvm-token')));
  console.log('hasTextNano', await page.getByText('NanoKVM').count());
  console.log('hasPasswordLabel', await page.getByText('Password', {exact:false}).count());
  const html = await page.evaluate(()=>document.documentElement.outerHTML); 
  console.log('contains login-form', html.includes('login-form'));
  await browser.close();
})();
