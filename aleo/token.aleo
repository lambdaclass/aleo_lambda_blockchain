program token.aleo;

 record token:
     owner as address.private;
     gates as u64.private;
     private_amount as u64.private;
     public_amount as u64.public;

 // create a new token record from an amount and the owner address
 function new:
     input r0 as u64.public;
     input r1 as address.public;
     cast r1 0u64 r0 r0 into r2 as token.record;      
     output r2 as token.record;
